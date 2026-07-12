package main

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"os"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"
)

// platformConfig holds the endpoints/credentials the worker uses to poll the
// Zot registry (via authd) for storage usage. Mirrors the env wiring the
// app uses (app/src/lib/zot.ts).
type platformConfig struct {
	authdURL       string
	registrySecret string
	registryURL    string
}

func loadPlatformConfig() platformConfig {
	authd := os.Getenv("AUTHD_INTERNAL_URL")
	if authd == "" {
		authd = "http://enzarb-authd.enzarb-system:8080"
	}
	registry := os.Getenv("REGISTRY_INTERNAL_URL")
	if registry == "" {
		registry = "http://zot.enzarb-system:5000"
	}
	return platformConfig{
		authdURL:       authd,
		registrySecret: os.Getenv("REGISTRY_ADMIN_TOKEN"),
		registryURL:    registry,
	}
}

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

	w := &Worker{db: pool, k8s: k8s, cfg: loadPlatformConfig(), http: &http.Client{Timeout: 30 * time.Second}}

	if len(os.Args) > 1 && os.Args[1] == "backfill-owners" {
		if err := w.backfillOwners(context.Background()); err != nil {
			slog.Error("backfill owners", "err", err)
			os.Exit(1)
		}
		return
	}

	// Metrics polling loop: pod compute/storage plus Zot registry storage.
	// Usage quantities are billed per the *actual* elapsed time since the
	// previous tick (not a fixed 60s), since collection itself takes time
	// and would otherwise shrink the real gap between samples below 60s,
	// undercounting usage for that period.
	//
	// Ticks are phase-locked to wall-clock minute boundaries rather than
	// free-running: recorded_at is truncated to the minute, and a
	// free-running ~60s ticker drifts across those boundaries, landing 0 or
	// 2 samples in a given minute bucket — which renders as a sawtooth in
	// the per-minute utilization charts.
	go func() {
		lastTick := time.Now()
		for {
			time.Sleep(time.Until(time.Now().Truncate(time.Minute).Add(time.Minute)))
			now := time.Now()
			elapsed := now.Sub(lastTick)
			lastTick = now
			slog.Info("collect metrics tick", "elapsed", elapsed)
			if err := w.collectMetrics(context.Background(), elapsed); err != nil {
				slog.Error("collect metrics", "err", err)
			}
			if err := w.collectZotUsage(context.Background(), elapsed); err != nil {
				slog.Error("collect zot usage", "err", err)
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
	db   *pgxpool.Pool
	k8s  *kubernetes.Clientset
	cfg  platformConfig
	http *http.Client

	// podOwners caches pod name → "Kind/name" owner string, populated each
	// metrics tick so the Hubble flush can attribute network bytes to the
	// same owner as the pod's compute events.
	podOwners   map[string]string
	podOwnersMu sync.RWMutex
}

// podOwner resolves the effective owner of a pod as "Kind/name". For pods
// owned by a ReplicaSet it walks up one level to return the Deployment (or
// whatever owns the RS). Returns "" if the pod has no owner references.
func (w *Worker) podOwner(ctx context.Context, ns string, pod *corev1.Pod) string {
	if len(pod.OwnerReferences) == 0 {
		return ""
	}
	ref := pod.OwnerReferences[0]
	if ref.Kind != "ReplicaSet" {
		return ref.Kind + "/" + stripHash(ref.Name, pod.Labels)
	}
	rs, err := w.k8s.AppsV1().ReplicaSets(ns).Get(ctx, ref.Name, metav1.GetOptions{})
	if err == nil && len(rs.OwnerReferences) > 0 {
		parent := rs.OwnerReferences[0]
		return parent.Kind + "/" + parent.Name
	}
	// The RS lookup failed (already garbage-collected) or it has no owner
	// ref of its own. Only the Deployment controller stamps a
	// pod-template-hash label onto a ReplicaSet's pods, so its presence
	// means this RS was Deployment-managed even though we can no longer
	// read the Deployment's name off it directly; strip the hash suffix to
	// recover the Deployment's name. A truly standalone ReplicaSet (no
	// Deployment ever involved) carries no such label and its name is
	// reported as-is.
	if hash := pod.Labels["pod-template-hash"]; hash != "" {
		return "Deployment/" + stripHash(ref.Name, pod.Labels)
	}
	return "ReplicaSet/" + ref.Name
}

// backfillOwners is a one-time data correction for usage_events rows written
// before podOwner learned to resolve a ReplicaSet's true parent. It re-derives
// each distinct "ReplicaSet/<name>" owner by looking the ReplicaSet up live in
// the cluster (cluster-wide, since usage_events doesn't retain namespace) and
// only rewrites it when that lookup gives a *confirmed* owner — never by
// guessing from the name, since not every ReplicaSet is Deployment-owned.
// Rows whose ReplicaSet has since been garbage-collected are left untouched:
// there's no live source of truth left to confirm them against.
func (w *Worker) backfillOwners(ctx context.Context) error {
	rows, err := w.db.Query(ctx, `SELECT DISTINCT owner FROM usage_events WHERE owner LIKE 'ReplicaSet/%'`)
	if err != nil {
		return fmt.Errorf("query distinct owners: %w", err)
	}
	var owners []string
	for rows.Next() {
		var o string
		if err := rows.Scan(&o); err != nil {
			rows.Close()
			return fmt.Errorf("scan owner: %w", err)
		}
		owners = append(owners, o)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return fmt.Errorf("iterate owners: %w", err)
	}

	var updated, unresolved int
	var stillUnresolved []string
	for _, owner := range owners {
		name := strings.TrimPrefix(owner, "ReplicaSet/")
		rsList, err := w.k8s.AppsV1().ReplicaSets(metav1.NamespaceAll).List(ctx, metav1.ListOptions{
			FieldSelector: "metadata.name=" + name,
		})
		if err != nil {
			slog.Warn("backfill: list replicasets", "name", name, "err", err)
			unresolved++
			stillUnresolved = append(stillUnresolved, owner)
			continue
		}
		if len(rsList.Items) == 0 || len(rsList.Items[0].OwnerReferences) == 0 {
			// Either already garbage-collected, or genuinely a standalone
			// ReplicaSet. The corroborated pass below may still resolve it.
			unresolved++
			stillUnresolved = append(stillUnresolved, owner)
			continue
		}
		parent := rsList.Items[0].OwnerReferences[0]
		newOwner := parent.Kind + "/" + parent.Name
		if newOwner == owner {
			continue
		}
		tag, err := w.db.Exec(ctx, `UPDATE usage_events SET owner = $1 WHERE owner = $2`, newOwner, owner)
		if err != nil {
			return fmt.Errorf("update owner %q -> %q: %w", owner, newOwner, err)
		}
		slog.Info("backfill: rewrote owner", "from", owner, "to", newOwner, "rows", tag.RowsAffected())
		updated++
	}

	corroborated, err := w.backfillUnresolvedByCorroboration(ctx, stillUnresolved)
	if err != nil {
		return err
	}
	slog.Info("backfill owners done", "distinct_owners", len(owners), "updated", updated,
		"corroborated", corroborated, "unresolved", unresolved-corroborated)
	return nil
}

// backfillUnresolvedByCorroboration handles ReplicaSets that no longer exist
// in the cluster (so podOwner/backfillOwners has no live source of truth to
// confirm them against) by cross-referencing already-confirmed owners in the
// same table. It strips the trailing pod-template-hash-shaped suffix from the
// ReplicaSet name and, only when that exact base name matches an owner this
// run (or a prior run) already confirmed live — e.g. "Deployment/krust-web"
// — rewrites to it. This is corroboration against real evidence recorded
// elsewhere in usage_events, not a guess: a row is only rewritten when
// another row for the very same workload was independently verified against
// the live cluster.
func (w *Worker) backfillUnresolvedByCorroboration(ctx context.Context, unresolvedOwners []string) (int, error) {
	if len(unresolvedOwners) == 0 {
		return 0, nil
	}

	rows, err := w.db.Query(ctx, `SELECT DISTINCT owner FROM usage_events WHERE owner NOT LIKE 'ReplicaSet/%'`)
	if err != nil {
		return 0, fmt.Errorf("query confirmed owners: %w", err)
	}
	confirmedByName := map[string]string{}
	for rows.Next() {
		var o string
		if err := rows.Scan(&o); err != nil {
			rows.Close()
			return 0, fmt.Errorf("scan confirmed owner: %w", err)
		}
		if i := strings.IndexByte(o, '/'); i > 0 {
			confirmedByName[o[i+1:]] = o
		}
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return 0, fmt.Errorf("iterate confirmed owners: %w", err)
	}

	var corroborated int
	for _, owner := range unresolvedOwners {
		name := strings.TrimPrefix(owner, "ReplicaSet/")
		base := stripHashSuffix(name)
		if base == name {
			continue
		}
		newOwner, ok := confirmedByName[base]
		if !ok || newOwner == owner {
			continue
		}
		tag, err := w.db.Exec(ctx, `UPDATE usage_events SET owner = $1 WHERE owner = $2`, newOwner, owner)
		if err != nil {
			return corroborated, fmt.Errorf("update owner %q -> %q: %w", owner, newOwner, err)
		}
		slog.Info("backfill: rewrote owner (corroborated)", "from", owner, "to", newOwner, "rows", tag.RowsAffected())
		corroborated++
	}
	return corroborated, nil
}

// hashSuffixRE matches a trailing "-<hash>" segment shaped like the
// lowercase-alphanumeric pod-template-hash Kubernetes appends to generated
// ReplicaSet names (typically 8-10 chars, occasionally shorter).
var hashSuffixRE = regexp.MustCompile(`-[a-z0-9]{6,10}$`)

// stripHashSuffix removes a trailing pod-template-hash-shaped suffix from
// name, if present. Returns name unchanged if it doesn't look like one.
func stripHashSuffix(name string) string {
	return hashSuffixRE.ReplaceAllString(name, "")
}

// stripHash removes the trailing "-<hash>" segment that Kubernetes appends to
// generated resource names, using the authoritative hash from the pod's labels
// (pod-template-hash for ReplicaSets, controller-revision-hash for
// StatefulSets). Names without a matching hash suffix are returned unchanged.
func stripHash(name string, labels map[string]string) string {
	for _, key := range []string{"pod-template-hash", "controller-revision-hash"} {
		if h := labels[key]; h != "" && strings.HasSuffix(name, "-"+h) {
			return strings.TrimSuffix(name, "-"+h)
		}
	}
	return name
}

// collectMetrics reads pod resource usage from the metrics-server and writes
// usage_events rows for both workspace pods (user-* namespaces) and deploy-
// environment pods (deploy-* namespaces).
func (w *Worker) collectMetrics(ctx context.Context, elapsed time.Duration) error {
	nsList, err := w.k8s.CoreV1().Namespaces().List(ctx, metav1.ListOptions{})
	if err != nil {
		return fmt.Errorf("list namespaces: %w", err)
	}

	now := time.Now().UTC().Truncate(time.Minute)
	for _, ns := range nsList.Items {
		switch {
		case strings.HasPrefix(ns.Name, "user-"):
			// Workspace: org UUID is encoded in the namespace name.
			orgID := strings.TrimPrefix(ns.Name, "user-")
			w.meterNamespacePods(ctx, ns.Name, orgID, "workspace", "", now, elapsed)
		case strings.HasPrefix(ns.Name, "deploy-"):
			// Deploy environment: org/project/env come from namespace labels (set
			// by the operator's EnvironmentReconciler). The namespace name itself
			// is a truncated/hashed identifier and isn't reliably parseable, so
			// deployEnvSlug is only a legacy fallback for namespaces predating the
			// env-slug label.
			orgID := ns.Labels["enzarb.io/org-id"]
			if orgID == "" {
				continue
			}
			envSlug := ns.Labels["enzarb.io/env-slug"]
			if envSlug == "" {
				envSlug = deployEnvSlug(ns.Name, orgID, ns.Labels["enzarb.io/project-slug"])
			}
			w.meterNamespacePods(ctx, ns.Name, orgID, "environment", envSlug, now, elapsed)
		}
	}
	return nil
}

// deployEnvSlug extracts the environment slug from a deploy namespace name of the
// form deploy-<orgID>-<projSlug>-<envSlug>. Falls back to the trailing segment.
func deployEnvSlug(nsName, orgID, projSlug string) string {
	prefix := "deploy-" + orgID + "-" + projSlug + "-"
	if projSlug != "" && strings.HasPrefix(nsName, prefix) {
		return strings.TrimPrefix(nsName, prefix)
	}
	parts := strings.Split(nsName, "-")
	return parts[len(parts)-1]
}

// meterNamespacePods records cpu/mem/storage usage for every metered pod
// in a namespace, attributed to the given component (and environment slug).
//
// For workspace namespaces (user-*) we filter by the operator label so we only
// meter the workspace pod itself, not any unrelated system pods.
// For deploy namespaces (deploy-*) we meter ALL running pods — they are
// tenant-deployed (via Helm etc.) and don't carry the operator label.
func (w *Worker) meterNamespacePods(ctx context.Context, nsName, orgID, component, environment string, now time.Time, elapsed time.Duration) {
	labelSel := ""
	if component == "workspace" {
		labelSel = "app.kubernetes.io/managed-by=enzarb-operator"
	}
	pods, err := w.k8s.CoreV1().Pods(nsName).List(ctx, metav1.ListOptions{
		LabelSelector: labelSel,
	})
	if err != nil {
		slog.Warn("list pods", "ns", nsName, "err", err)
		return
	}

	// For deploy namespaces the project slug comes from the namespace label, not
	// the pod — tenant pods don't carry enzarb.io/project labels.
	nsProjSlug := ""
	if component == "environment" {
		nsMeta, err2 := w.k8s.CoreV1().Namespaces().Get(ctx, nsName, metav1.GetOptions{})
		if err2 == nil {
			nsProjSlug = nsMeta.Labels["enzarb.io/project-slug"]
		}
	}

	for _, pod := range pods.Items {
		// Skip pods that aren't running yet — they consume no real resources.
		if pod.Status.Phase != corev1.PodRunning {
			continue
		}

		projectSlug := pod.Labels["enzarb.io/project"]
		if projectSlug == "" {
			projectSlug = pod.Labels["enzarb.io/project-slug"]
		}
		// For deploy-namespace pods fall back to the namespace-level project slug.
		if projectSlug == "" {
			projectSlug = nsProjSlug
		}
		if projectSlug == "" {
			continue
		}

		// Resolve pod owner (Deployment, StatefulSet, etc.) for billing grouping.
		// Cache in podOwners so the Hubble consumer can attribute network bytes too.
		owner := w.podOwner(ctx, nsName, &pod)
		w.podOwnersMu.Lock()
		if w.podOwners == nil {
			w.podOwners = map[string]string{}
		}
		w.podOwners[pod.Name] = owner
		w.podOwnersMu.Unlock()

		// Get metrics from metrics-server via SubResource
		podMetrics, err := w.getPodMetrics(ctx, nsName, pod.Name)
		if err != nil {
			slog.Warn("get pod metrics", "pod", pod.Name, "err", err)
			continue
		}

		cpuMillis, memBytes := podMetrics.cpu, podMetrics.mem
		elapsedHours := elapsed.Hours()
		// Convert to standard billing units over the actual elapsed tick interval
		// (not a fixed 60s, since collection time eats into the real gap).
		// vCPU-hours: cores * elapsed hours
		vcpuHours := float64(cpuMillis) / 1000.0 * elapsedHours
		// GiB-hours: GiB * elapsed hours
		memGiBHours := float64(memBytes) / (1024 * 1024 * 1024) * elapsedHours

		// Block storage: record each PVC separately so the UI can show per-volume costs.
		// 1 GiB-month = 30d × 24h = 720 GiB-hours.
		pvcs, err := w.k8s.CoreV1().PersistentVolumeClaims(nsName).List(ctx, metav1.ListOptions{
			LabelSelector: fmt.Sprintf("enzarb.io/project=%s", projectSlug),
		})
		if err == nil {
			for _, pvc := range pvcs.Items {
				if q, ok := pvc.Spec.Resources.Requests[corev1.ResourceStorage]; ok {
					pvcGiB := float64(q.Value()) / (1024 * 1024 * 1024)
					pvcGiBMonths := pvcGiB * elapsedHours / 720.0
					if err := w.insertUsage(ctx, orgID, projectSlug, component, environment, pvc.Name, owner, "block_storage_gib_months", pvcGiBMonths, "GiB-mo", now); err != nil {
						slog.Warn("insert storage usage", "err", err)
					}
				}
			}
		}

		if err := w.insertUsage(ctx, orgID, projectSlug, component, environment, pod.Name, owner, "vcpu_hours", vcpuHours, "vCPU-hr", now); err != nil {
			slog.Warn("insert cpu usage", "err", err)
		}
		if err := w.insertUsage(ctx, orgID, projectSlug, component, environment, pod.Name, owner, "mem_gib_hours", memGiBHours, "GiB-hr", now); err != nil {
			slog.Warn("insert mem usage", "err", err)
		}
	}
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

// insertUsage records a usage event. orgID is the organization's UUID, which is
// what the `user-<orgID>` namespace encodes — match on it directly. (Matching on
// slug here silently inserted zero rows, since the namespace carries the id.)
// label is fine-grained attribution (pod name, PVC name, image path).
// owner is the K8s owner of the label (e.g. "Deployment/my-app", "StatefulSet/db").
func (w *Worker) insertUsage(ctx context.Context, orgID, projectSlug, component, environment, label, owner, resourceType string, quantity float64, unit string, at time.Time) error {
	var env any
	if environment != "" {
		env = environment
	}
	var lbl any
	if label != "" {
		lbl = label
	}
	var own any
	if owner != "" {
		own = owner
	}
	tag, err := w.db.Exec(ctx, `
		INSERT INTO usage_events (org_id, project_id, component, environment, label, owner, resource_type, quantity, unit, recorded_at)
		SELECT id, $2, $3, $4, $5, $6, $7, $8, $9, $10
		FROM organizations WHERE id = $1::uuid
	`, orgID, projectSlug, component, env, lbl, own, resourceType, quantity, unit, at)
	if err == nil && tag.RowsAffected() == 0 {
		slog.Warn("insertUsage matched no org", "orgID", orgID, "resourceType", resourceType)
	}
	return err
}

// HubbleEvent is the outer wrapper Hubble writes to the file export log.
// The actual flow is nested under the "flow" key.
type HubbleEvent struct {
	Flow *HubbleFlow `json:"flow"`
}

// HubbleFlow is a minimal subset of Hubble's JSON flow export format.
type HubbleFlow struct {
	Source      *HubbleEndpoint `json:"source"`
	Destination *HubbleEndpoint `json:"destination"`
	Type        string          `json:"type"`
	L4          *HubbleL4       `json:"l4"`
	Verdict     string          `json:"verdict"`
	IP          *HubbleIP       `json:"IP"`
}

type HubbleIP struct {
	Source      string `json:"source"`
	Destination string `json:"destination"`
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

	// Track per-(org, project, pod, component, external) byte counts and flush
	// every 60s. Flows are split into internal (RFC-1918 peer) and external
	// (public internet peer) so billing can price them differently.
	type flowKey struct {
		orgID, project, pod, component string
		external                       bool
	}
	type byteCounts struct{ ingress, egress int64 }
	counts := map[flowKey]*byteCounts{}
	var flowsSeen, flowsMatched int64

	// Phase-locked to minute boundaries for the same reason as the metrics
	// loop: recorded_at is minute-truncated, and a free-running ticker lands
	// 0 or 2 flushes in some minutes, sawtoothing the network charts.
	go func() {
		for {
			time.Sleep(time.Until(time.Now().Truncate(time.Minute).Add(time.Minute)))
			slog.Info("hubble flush", "flows_seen", flowsSeen, "flows_matched", flowsMatched, "buckets", len(counts))
			flowsSeen, flowsMatched = 0, 0
			for key, bc := range counts {
				w.podOwnersMu.RLock()
				owner := w.podOwners[key.pod]
				w.podOwnersMu.RUnlock()
				slog.Info("hubble bucket", "orgID", key.orgID, "project", key.project, "pod", key.pod, "owner", owner, "component", key.component, "external", key.external, "ingress", bc.ingress, "egress", bc.egress)
				now := time.Now().UTC().Truncate(time.Minute)
				ingressType, egressType := "net_ingress_internal_bytes", "net_egress_internal_bytes"
				if key.external {
					ingressType, egressType = "net_ingress_external_bytes", "net_egress_external_bytes"
				}
				if bc.ingress > 0 {
					if err := w.insertUsage(ctx, key.orgID, key.project, key.component, "", key.pod, owner, ingressType, float64(bc.ingress), "bytes", now); err != nil {
						slog.Warn("insert ingress usage", "err", err)
					}
				}
				if bc.egress > 0 {
					if err := w.insertUsage(ctx, key.orgID, key.project, key.component, "", key.pod, owner, egressType, float64(bc.egress), "bytes", now); err != nil {
						slog.Warn("insert egress usage", "err", err)
					}
				}
			}
			counts = map[flowKey]*byteCounts{}
		}
	}()

	// processLine parses one JSON line and accumulates flow bytes.
	processLine := func(line []byte) {
		var event HubbleEvent
		if err := json.Unmarshal(line, &event); err != nil {
			return
		}
		flow := event.Flow
		if flow == nil || flow.Verdict != "FORWARDED" {
			return
		}
		flowsSeen++

		srcOrg, srcProject := endpointOrgProject(flow.Source)
		dstOrg, dstProject := endpointOrgProject(flow.Destination)

		peerIP := ""
		if flow.IP != nil {
			if srcProject != "" {
				peerIP = flow.IP.Destination
			} else {
				peerIP = flow.IP.Source
			}
		}
		external := !isInternalIP(peerIP)

		var key flowKey
		egress := false
		switch {
		case srcProject != "":
			pod := ""
			if flow.Source != nil {
				pod = flow.Source.PodName
			}
			key, egress = flowKey{srcOrg, srcProject, pod, peerComponent(flow.Destination), external}, true
		case dstProject != "":
			pod := ""
			if flow.Destination != nil {
				pod = flow.Destination.PodName
			}
			key = flowKey{dstOrg, dstProject, pod, peerComponent(flow.Source), external}
		default:
			return
		}
		if key.orgID == "" {
			return
		}
		flowsMatched++

		if _, ok := counts[key]; !ok {
			counts[key] = &byteCounts{}
		}
		if egress {
			counts[key].egress += 1500 // approximate MTU per flow record
		} else {
			counts[key].ingress += 1500
		}
	}

	// tailFile reads all new lines from f (from current position to EOF),
	// sleeping on EOF. Returns true when the file at path has been rotated
	// (different inode), signalling the caller to open a fresh file.
	tailFile := func(f *os.File) bool {
		reader := bufio.NewReaderSize(f, 256*1024)
		var partial []byte
		eofSleeps := 0
		for {
			if ctx.Err() != nil {
				return false
			}
			line, err := reader.ReadBytes('\n')
			if len(line) > 0 {
				eofSleeps = 0
				partial = append(partial, line...)
				if partial[len(partial)-1] == '\n' {
					processLine(partial[:len(partial)-1])
					partial = partial[:0]
				}
			}
			if err == io.EOF {
				// After a short wait, check whether the file has been rotated.
				// Only check after eofSleeps > 0 so we don't misfire on the
				// very first read when the file hasn't changed yet.
				time.Sleep(50 * time.Millisecond)
				eofSleeps++
				if eofSleeps >= 2 {
					if fi1, e1 := f.Stat(); e1 == nil {
						if fi2, e2 := os.Stat(path); e2 == nil && !os.SameFile(fi1, fi2) { //nolint:gosec // path is from env config, not user input
							return true // rotated
						}
					}
				}
				continue
			}
			if err != nil {
				return false
			}
		}
	}

	var hubbleMissing bool
	firstOpen := true
	for {
		if ctx.Err() != nil {
			return
		}

		f, err := os.Open(path) //nolint:gosec // path is from env config, not user input
		if err != nil {
			if !hubbleMissing {
				slog.Warn("hubble flow log unavailable — network usage will not appear in billing; deploy Cilium with Hubble JSON flow export to enable", "path", path, "err", err)
				hubbleMissing = true
			}
			time.Sleep(5 * time.Second)
			continue
		}
		hubbleMissing = false

		// On first open only: seek to end so we don't replay all history.
		// On rotation-triggered reopens: read from the beginning of the new
		// file so we capture everything written to it before we got there.
		if firstOpen {
			if _, err := f.Seek(0, io.SeekEnd); err != nil {
				slog.Warn("seek hubble log", "err", err)
				_ = f.Close()
				continue
			}
			firstOpen = false
		}

		tailFile(f)
		_ = f.Close()
	}
}

// internalCIDRs covers RFC-1918, loopback, link-local, and the Kubernetes
// default pod/service ranges. Any peer IP in these blocks is "internal";
// everything else is public internet ("external").
var internalCIDRs = func() []*net.IPNet {
	cidrs := []string{
		"10.0.0.0/8",
		"172.16.0.0/12",
		"192.168.0.0/16",
		"127.0.0.0/8",
		"169.254.0.0/16",
		"::1/128",
		"fc00::/7",
	}
	nets := make([]*net.IPNet, 0, len(cidrs))
	for _, c := range cidrs {
		_, n, _ := net.ParseCIDR(c)
		nets = append(nets, n)
	}
	return nets
}()

// isInternalIP reports whether ip is an RFC-1918 / loopback / link-local address.
// An empty or unparseable string returns true (conservatively treated as internal).
func isInternalIP(ip string) bool {
	if ip == "" {
		return true
	}
	parsed := net.ParseIP(ip)
	if parsed == nil {
		return true
	}
	for _, n := range internalCIDRs {
		if n.Contains(parsed) {
			return true
		}
	}
	return false
}

// endpointOrgProject returns the org UUID and project slug for an endpoint, or
// empty strings if it isn't a project pod. Handles two namespace patterns:
//   - user-<orgID>: workspace pods; org derived from namespace prefix, project from pod label
//   - deploy-*: environment pods; org/project from namespace labels propagated by Cilium
func endpointOrgProject(ep *HubbleEndpoint) (orgID, project string) {
	if ep == nil {
		return "", ""
	}
	switch {
	case strings.HasPrefix(ep.Namespace, "user-"):
		project = extractProjectSlug(ep)
		if project != "" {
			orgID = strings.TrimPrefix(ep.Namespace, "user-")
		}
	case strings.HasPrefix(ep.Namespace, "deploy-"):
		// Cilium propagates namespace labels into endpoint labels with the
		// "k8s:io.cilium.k8s.namespace.labels." prefix.
		for _, label := range ep.Labels {
			if v, ok := strings.CutPrefix(label, "k8s:io.cilium.k8s.namespace.labels.enzarb.io/org-id="); ok {
				orgID = v
			}
			if v, ok := strings.CutPrefix(label, "k8s:io.cilium.k8s.namespace.labels.enzarb.io/project-slug="); ok {
				project = v
			}
		}
	}
	return orgID, project
}

// peerComponent classifies the non-project side of a flow so bandwidth to the
// platform's Zot registry backend is attributed to that component. Anything else
// (project egress to the internet, between project pods) counts as "workspace".
func peerComponent(ep *HubbleEndpoint) string {
	if ep == nil || ep.Namespace != "enzarb-system" {
		return "workspace"
	}
	switch {
	case strings.HasPrefix(ep.PodName, "zot"):
		return "zot"
	default:
		return "workspace"
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

// parseQuantityMillis parses a k8s CPU quantity string into millicores.
// The metrics-server returns live CPU as nanocores ("42381456n"); static
// resource specs use millicores ("250m") or whole cores ("1").
func parseQuantityMillis(s string) int64 {
	s = strings.TrimSpace(s)
	if strings.HasSuffix(s, "n") {
		// nanocores → millicores (divide by 1,000,000)
		v, _ := strconv.ParseInt(strings.TrimSuffix(s, "n"), 10, 64)
		return v / 1_000_000
	}
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

const gib = 1024 * 1024 * 1024

// orgIDBySlug loads a slug→UUID map for all organizations, so Zot registry usage
// (which is keyed by org slug) can be attributed to the right org row.
func (w *Worker) orgIDBySlug(ctx context.Context) (map[string]string, error) {
	rows, err := w.db.Query(ctx, `SELECT id, slug FROM organizations`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	m := map[string]string{}
	for rows.Next() {
		var id, slug string
		if err := rows.Scan(&id, &slug); err != nil {
			return nil, err
		}
		m[slug] = id
	}
	return m, rows.Err()
}

// collectZotUsage lists registry repositories and sums distinct blob sizes per
// repo (deduped by digest), recording registry_gib_months. Repo paths follow
// the <orgSlug>/<image> convention, so the first segment maps to the org and the
// remainder is used as the project_id. Mirrors app/src/lib/zot.ts auth.
func (w *Worker) collectZotUsage(ctx context.Context, elapsed time.Duration) error {
	if w.cfg.registrySecret == "" {
		return nil // not configured; skip silently
	}
	orgIDs, err := w.orgIDBySlug(ctx)
	if err != nil {
		return fmt.Errorf("org ids: %w", err)
	}
	now := time.Now().UTC().Truncate(time.Minute)

	var catalog struct {
		Repositories []string `json:"repositories"`
	}
	// Zot v2 uses 'repository::pull' (empty repo name) to authorize _catalog,
	// not the Docker spec's 'registry:catalog:*'. Use the scope Zot challenges with.
	if _, err := w.zotGet(ctx, "/v2/_catalog", "repository::pull", &catalog); err != nil {
		return fmt.Errorf("catalog: %w", err)
	}

	slog.Info("zot catalog", "repos", len(catalog.Repositories))
	for _, repo := range catalog.Repositories {
		// Repo paths are orgSlug/projectSlug/imageName; extract just the first two
		// segments. strings.Cut would give projectSlug/imageName as the "project",
		// which doesn't match the project slug in the DB.
		parts := strings.SplitN(repo, "/", 3)
		if len(parts) < 2 {
			continue
		}
		orgSlug, project := parts[0], parts[1]
		orgID, ok := orgIDs[orgSlug]
		if !ok {
			slog.Warn("zot repo org not found", "repo", repo, "orgSlug", orgSlug)
			continue // repo not owned by a known org
		}
		sizeBytes, err := w.zotRepoSize(ctx, repo)
		if err != nil {
			slog.Warn("zot repo size", "repo", repo, "err", err)
			continue
		}
		sizeGiB := float64(sizeBytes) / gib
		// 1 GiB-month = 720 GiB-hours.
		sizeGiBMonths := sizeGiB * elapsed.Hours() / 720.0
		// Use the image name (everything after orgSlug/projectSlug/) as the label so
		// the UI can show per-image registry costs. For 2-segment paths the image is
		// the project slug itself (no sub-path), so label falls back to "".
		imageName := ""
		if len(parts) >= 3 {
			imageName = parts[2]
		}
		slog.Info("zot repo usage", "repo", repo, "sizeGiB", sizeGiB)
		if err := w.insertUsage(ctx, orgID, project, "zot", "", imageName, "", "registry_gib_months", sizeGiBMonths, "GiB-mo", now); err != nil {
			slog.Warn("insert zot usage", "err", err)
		}
	}
	return nil
}

// zotRepoSize sums the size of every distinct blob (config + layers, deduped by
// digest across all tags) referenced by a repository's manifests. Handles both
// single-arch manifests and OCI image indexes (multi-platform manifest lists).
func (w *Worker) zotRepoSize(ctx context.Context, repo string) (int64, error) {
	scope := "repository:" + repo + ":pull"
	var tags struct {
		Tags []string `json:"tags"`
	}
	if status, err := w.zotGet(ctx, "/v2/"+repo+"/tags/list", scope, &tags); err != nil {
		return 0, err
	} else if status == http.StatusNotFound {
		return 0, nil
	}

	type descriptor struct {
		MediaType string `json:"mediaType"`
		Digest    string `json:"digest"`
		Size      int64  `json:"size"`
	}
	type manifestDoc struct {
		MediaType string       `json:"mediaType"`
		Config    descriptor   `json:"config"`
		Layers    []descriptor `json:"layers"`
		Manifests []descriptor `json:"manifests"` // OCI index / manifest list
	}

	seen := map[string]int64{}

	var accumulateManifest func(ref string)
	accumulateManifest = func(ref string) {
		var m manifestDoc
		if status, err := w.zotGet(ctx, "/v2/"+repo+"/manifests/"+ref, scope, &m); err != nil {
			slog.Warn("zot manifest", "repo", repo, "ref", ref, "err", err)
			return
		} else if status == http.StatusNotFound {
			return
		}
		// OCI image index or Docker manifest list — recurse into children.
		if len(m.Manifests) > 0 {
			for _, child := range m.Manifests {
				accumulateManifest(child.Digest)
			}
			return
		}
		// Single-arch image manifest — accumulate blobs.
		if m.Config.Digest != "" {
			seen[m.Config.Digest] = m.Config.Size
		}
		for _, l := range m.Layers {
			seen[l.Digest] = l.Size
		}
	}

	for _, tag := range tags.Tags {
		accumulateManifest(tag)
	}

	var total int64
	for _, sz := range seen {
		total += sz
	}
	return total, nil
}

func (w *Worker) zotGet(ctx context.Context, path, scope string, out any) (int, error) {
	token, err := w.registryToken(ctx, scope)
	if err != nil {
		return 0, fmt.Errorf("registry token: %w", err)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, w.cfg.registryURL+path, nil)
	if err != nil {
		return 0, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	// Accept both OCI and Docker v2 manifest media types.
	req.Header.Set("Accept", "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json")
	resp, err := w.http.Do(req)
	if err != nil {
		return 0, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode == http.StatusNotFound {
		return resp.StatusCode, nil
	}
	if resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return resp.StatusCode, fmt.Errorf("registry %d: %s", resp.StatusCode, body)
	}
	if err := json.NewDecoder(resp.Body).Decode(out); err != nil {
		return resp.StatusCode, err
	}
	return resp.StatusCode, nil
}

// registryToken mints a scoped bearer token from authd, authenticating as the
// shared "admin" identity (matching app/src/lib/zot.ts registryToken).
func (w *Worker) registryToken(ctx context.Context, scope string) (string, error) {
	q := url.Values{"service": {"registry.enzarb.dev"}, "scope": {scope}}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, w.cfg.authdURL+"/auth/token?"+q.Encode(), nil)
	if err != nil {
		return "", err
	}
	creds := base64.StdEncoding.EncodeToString([]byte("admin:" + w.cfg.registrySecret))
	req.Header.Set("Authorization", "Basic "+creds)
	resp, err := w.http.Do(req)
	if err != nil {
		return "", err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return "", fmt.Errorf("authd %d: %s", resp.StatusCode, body)
	}
	var tok struct {
		Token       string `json:"token"`
		AccessToken string `json:"access_token"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&tok); err != nil {
		return "", err
	}
	if tok.Token != "" {
		return tok.Token, nil
	}
	return tok.AccessToken, nil
}
