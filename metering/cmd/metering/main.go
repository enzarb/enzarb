package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"
)

func main() {
	slog.Info("metering worker starting")

	dbURL := os.Getenv("DATABASE_URL")
	if dbURL == "" {
		slog.Error("DATABASE_URL required")
		os.Exit(1)
	}

	pool, err := pgxpool.New(context.Background(), dbURL)
	if err != nil {
		slog.Error("db connect", "err", err)
		os.Exit(1)
	}
	defer pool.Close()

	cfg, err := rest.InClusterConfig()
	if err != nil {
		slog.Error("k8s config", "err", err)
		os.Exit(1)
	}
	k8s, err := kubernetes.NewForConfig(cfg)
	if err != nil {
		slog.Error("k8s client", "err", err)
		os.Exit(1)
	}

	w := &Worker{db: pool, k8s: k8s}

	// Metrics polling loop
	go func() {
		ticker := time.NewTicker(60 * time.Second)
		defer ticker.Stop()
		for range ticker.C {
			if err := w.collectMetrics(context.Background()); err != nil {
				slog.Error("collect metrics", "err", err)
			}
		}
	}()

	// Hubble flow log consumer
	hubblePath := os.Getenv("HUBBLE_LOG_PATH")
	if hubblePath == "" {
		hubblePath = "/var/run/cilium/hubble/events.log"
	}
	w.consumeHubble(context.Background(), hubblePath)
}

type Worker struct {
	db  *pgxpool.Pool
	k8s *kubernetes.Clientset
}

// collectMetrics reads pod resource usage from the metrics-server and writes usage_events rows.
func (w *Worker) collectMetrics(ctx context.Context) error {
	// List all pods in user-* namespaces
	nsList, err := w.k8s.CoreV1().Namespaces().List(ctx, metav1.ListOptions{})
	if err != nil {
		return fmt.Errorf("list namespaces: %w", err)
	}

	now := time.Now().UTC()
	for _, ns := range nsList.Items {
		if !strings.HasPrefix(ns.Name, "user-") {
			continue
		}
		orgID := strings.TrimPrefix(ns.Name, "user-")

		pods, err := w.k8s.CoreV1().Pods(ns.Name).List(ctx, metav1.ListOptions{
			LabelSelector: "app.kubernetes.io/managed-by=enzarb-operator",
		})
		if err != nil {
			slog.Warn("list pods", "ns", ns.Name, "err", err)
			continue
		}

		for _, pod := range pods.Items {
			projectSlug := pod.Labels["enzarb.io/project"]
			if projectSlug == "" {
				continue
			}

			// Get metrics from metrics-server via SubResource
			podMetrics, err := w.getPodMetrics(ctx, ns.Name, pod.Name)
			if err != nil {
				slog.Warn("get pod metrics", "pod", pod.Name, "err", err)
				continue
			}

			cpuMillis, memBytes := podMetrics.cpu, podMetrics.mem
			// Convert to CPU-seconds (60s interval) and mem GiB-seconds
			cpuSeconds := float64(cpuMillis) / 1000.0 * 60.0
			memGiBSeconds := float64(memBytes) / (1024 * 1024 * 1024) * 60.0

			// Storage: sum PVC capacity
			pvcs, err := w.k8s.CoreV1().PersistentVolumeClaims(ns.Name).List(ctx, metav1.ListOptions{
				LabelSelector: fmt.Sprintf("enzarb.io/project=%s", projectSlug),
			})
			if err == nil {
				storageGiB := 0.0
				for _, pvc := range pvcs.Items {
					if q, ok := pvc.Spec.Resources.Requests[corev1.ResourceStorage]; ok {
						storageGiB += float64(q.Value()) / (1024 * 1024 * 1024)
					}
				}
				storageGiBSeconds := storageGiB * 60.0
				if err := w.insertUsage(ctx, orgID, projectSlug, "storage_gib_seconds", storageGiBSeconds, "GiB-s", now); err != nil {
					slog.Warn("insert storage usage", "err", err)
				}
			}

			if err := w.insertUsage(ctx, orgID, projectSlug, "cpu_seconds", cpuSeconds, "cpu-s", now); err != nil {
				slog.Warn("insert cpu usage", "err", err)
			}
			if err := w.insertUsage(ctx, orgID, projectSlug, "mem_gib_seconds", memGiBSeconds, "GiB-s", now); err != nil {
				slog.Warn("insert mem usage", "err", err)
			}
		}
	}
	return nil
}

type podMetricsResult struct {
	cpu int64 // millicores
	mem int64 // bytes
}

func (w *Worker) getPodMetrics(ctx context.Context, ns, name string) (*podMetricsResult, error) {
	// Use raw metrics-server API since we don't import metrics client
	data, err := w.k8s.RESTClient().Get().
		AbsPath(fmt.Sprintf("/apis/metrics.k8s.io/v1beta1/namespaces/%s/pods/%s", ns, name)).
		DoRaw(ctx)
	if err != nil {
		return nil, err
	}

	var obj struct {
		Containers []struct {
			Usage struct {
				CPU    string `json:"cpu"`
				Memory string `json:"memory"`
			} `json:"usage"`
		} `json:"containers"`
	}
	if err := json.Unmarshal(data, &obj); err != nil {
		return nil, err
	}

	var totalCPU, totalMem int64
	for _, c := range obj.Containers {
		totalCPU += parseQuantityMillis(c.Usage.CPU)
		totalMem += parseQuantityBytes(c.Usage.Memory)
	}
	return &podMetricsResult{cpu: totalCPU, mem: totalMem}, nil
}

func (w *Worker) insertUsage(ctx context.Context, orgID, projectSlug, resourceType string, quantity float64, unit string, at time.Time) error {
	_, err := w.db.Exec(ctx, `
		INSERT INTO usage_events (org_id, project_id, resource_type, quantity, unit, recorded_at)
		SELECT id, $2, $3, $4, $5, $6
		FROM organizations WHERE slug = $1
	`, orgID, projectSlug, resourceType, quantity, unit, at)
	return err
}

// HubbleFlow is a minimal subset of Hubble's JSON flow export format.
type HubbleFlow struct {
	Source      *HubbleEndpoint `json:"source"`
	Destination *HubbleEndpoint `json:"destination"`
	Type        string          `json:"type"`
	L4          *HubbleL4       `json:"l4"`
	Verdict     string          `json:"verdict"`
}

type HubbleEndpoint struct {
	Namespace string   `json:"namespace"`
	PodName   string   `json:"pod_name"`
	Labels    []string `json:"labels"`
}

type HubbleL4 struct {
	TCP *struct {
		SourcePort int `json:"source_port"`
		DestPort   int `json:"destination_port"`
	} `json:"TCP"`
	UDP *struct {
		SourcePort int `json:"source_port"`
		DestPort   int `json:"destination_port"`
	} `json:"UDP"`
}

// consumeHubble tails the Hubble JSON flow log and accumulates ingress/egress bytes.
func (w *Worker) consumeHubble(ctx context.Context, path string) {
	slog.Info("consuming hubble flows", "path", path)

	// Track per-project byte counts and flush every 60s
	type byteCounts struct{ ingress, egress int64 }
	counts := map[string]*byteCounts{}

	flush := time.NewTicker(60 * time.Second)
	defer flush.Stop()

	go func() {
		for range flush.C {
			for projectSlug, bc := range counts {
				now := time.Now().UTC()
				// Org lookup is handled inside insertUsage via SQL join
				// For Hubble flows we tag by pod label enzarb.io/project
				if bc.ingress > 0 {
					if err := w.insertUsage(ctx, "_hubble", projectSlug, "net_ingress_bytes", float64(bc.ingress), "bytes", now); err != nil {
						slog.Warn("insert ingress usage", "err", err)
					}
				}
				if bc.egress > 0 {
					if err := w.insertUsage(ctx, "_hubble", projectSlug, "net_egress_bytes", float64(bc.egress), "bytes", now); err != nil {
						slog.Warn("insert egress usage", "err", err)
					}
				}
			}
			counts = map[string]*byteCounts{}
		}
	}()

	for {
		f, err := os.Open(path) //nolint:gosec // path is from env config, not user input
		if err != nil {
			slog.Warn("open hubble log", "err", err)
			time.Sleep(5 * time.Second)
			continue
		}

		// Seek to end, then tail
		if _, err := f.Seek(0, 2); err != nil {
			slog.Warn("seek hubble log", "err", err)
			_ = f.Close()
			continue
		}
		scanner := bufio.NewScanner(f)
		for scanner.Scan() {
			if ctx.Err() != nil {
				_ = f.Close()
				return
			}
			var flow HubbleFlow
			if err := json.Unmarshal(scanner.Bytes(), &flow); err != nil {
				continue
			}
			if flow.Verdict != "FORWARDED" {
				continue
			}

			// Identify project from source/dest pod labels
			projectSlug := extractProjectSlug(flow.Source)
			if projectSlug == "" {
				projectSlug = extractProjectSlug(flow.Destination)
			}
			if projectSlug == "" {
				continue
			}

			if _, ok := counts[projectSlug]; !ok {
				counts[projectSlug] = &byteCounts{}
			}

			// Heuristic: if source pod is the project, this is egress; if dest, ingress
			if extractProjectSlug(flow.Source) == projectSlug {
				counts[projectSlug].egress += 1500 // approximate MTU per flow record
			} else {
				counts[projectSlug].ingress += 1500
			}
		}
		_ = f.Close()
		time.Sleep(100 * time.Millisecond)
	}
}

func extractProjectSlug(ep *HubbleEndpoint) string {
	if ep == nil {
		return ""
	}
	for _, label := range ep.Labels {
		if strings.HasPrefix(label, "k8s:enzarb.io/project=") {
			return strings.TrimPrefix(label, "k8s:enzarb.io/project=")
		}
	}
	return ""
}

// parseQuantityMillis parses a k8s CPU quantity string (e.g. "250m", "1") into millicores.
func parseQuantityMillis(s string) int64 {
	s = strings.TrimSpace(s)
	if strings.HasSuffix(s, "m") {
		v, _ := strconv.ParseInt(strings.TrimSuffix(s, "m"), 10, 64)
		return v
	}
	v, _ := strconv.ParseFloat(s, 64)
	return int64(v * 1000)
}

// parseQuantityBytes parses a k8s memory quantity string (e.g. "256Mi", "1Gi") into bytes.
func parseQuantityBytes(s string) int64 {
	s = strings.TrimSpace(s)
	multipliers := map[string]int64{
		"Ki": 1024, "Mi": 1024 * 1024, "Gi": 1024 * 1024 * 1024,
		"K": 1000, "M": 1000 * 1000, "G": 1000 * 1000 * 1000,
	}
	for suffix, mult := range multipliers {
		if strings.HasSuffix(s, suffix) {
			v, _ := strconv.ParseInt(strings.TrimSuffix(s, suffix), 10, 64)
			return v * mult
		}
	}
	v, _ := strconv.ParseInt(s, 10, 64)
	return v
}
