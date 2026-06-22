package main

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
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

// platformConfig holds the endpoints/credentials the worker uses to poll Gitea
// and the Zot registry (via authd) for storage usage. Mirrors the env wiring the
// app uses (app/src/lib/gitea.ts, app/src/lib/zot.ts).
type platformConfig struct {
	giteaURL       string
	giteaToken     string
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
		giteaURL:       os.Getenv("GITEA_URL"),
		giteaToken:     os.Getenv("GITEA_ADMIN_TOKEN"),
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

	// Metrics polling loop: pod compute/storage plus Gitea/Zot storage.
	go func() {
		ticker := time.NewTicker(60 * time.Second)
		defer ticker.Stop()
		for range ticker.C {
			if err := w.collectMetrics(context.Background()); err != nil {
				slog.Error("collect metrics", "err", err)
			}
			if err := w.collectGiteaUsage(context.Background()); err != nil {
				slog.Error("collect gitea usage", "err", err)
			}
			if err := w.collectZotUsage(context.Background()); err != nil {
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
}

// collectMetrics reads pod resource usage from the metrics-server and writes
// usage_events rows for both workspace pods (user-* namespaces) and deploy-
// environment pods (deploy-* namespaces).
func (w *Worker) collectMetrics(ctx context.Context) error {
	nsList, err := w.k8s.CoreV1().Namespaces().List(ctx, metav1.ListOptions{})
	if err != nil {
		return fmt.Errorf("list namespaces: %w", err)
	}

	now := time.Now().UTC()
	for _, ns := range nsList.Items {
		switch {
		case strings.HasPrefix(ns.Name, "user-"):
			// Workspace: org UUID is encoded in the namespace name.
			orgID := strings.TrimPrefix(ns.Name, "user-")
			w.meterNamespacePods(ctx, ns.Name, orgID, "workspace", "", now)
		case strings.HasPrefix(ns.Name, "deploy-"):
			// Deploy environment: org/project come from namespace labels (set by
			// the operator's EnvironmentReconciler); env slug is the trailing
			// segment of deploy-<orgID>-<projSlug>-<envSlug>.
			orgID := ns.Labels["enzarb.io/org-id"]
			if orgID == "" {
				continue
			}
			envSlug := deployEnvSlug(ns.Name, orgID, ns.Labels["enzarb.io/project-slug"])
			w.meterNamespacePods(ctx, ns.Name, orgID, "environment", envSlug, now)
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

// meterNamespacePods records cpu/mem/storage usage for every enzarb-managed pod
// in a namespace, attributed to the given component (and environment slug).
func (w *Worker) meterNamespacePods(ctx context.Context, nsName, orgID, component, environment string, now time.Time) {
	pods, err := w.k8s.CoreV1().Pods(nsName).List(ctx, metav1.ListOptions{
		LabelSelector: "app.kubernetes.io/managed-by=enzarb-operator",
	})
	if err != nil {
		slog.Warn("list pods", "ns", nsName, "err", err)
		return
	}

	for _, pod := range pods.Items {
		projectSlug := pod.Labels["enzarb.io/project"]
		if projectSlug == "" {
			projectSlug = pod.Labels["enzarb.io/project-slug"]
		}
		if projectSlug == "" {
			continue
		}

		// Get metrics from metrics-server via SubResource
		podMetrics, err := w.getPodMetrics(ctx, nsName, pod.Name)
		if err != nil {
			slog.Warn("get pod metrics", "pod", pod.Name, "err", err)
			continue
		}

		cpuMillis, memBytes := podMetrics.cpu, podMetrics.mem
		// Convert to CPU-seconds (60s interval) and mem GiB-seconds
		cpuSeconds := float64(cpuMillis) / 1000.0 * 60.0
		memGiBSeconds := float64(memBytes) / (1024 * 1024 * 1024) * 60.0

		// Storage: sum PVC capacity
		pvcs, err := w.k8s.CoreV1().PersistentVolumeClaims(nsName).List(ctx, metav1.ListOptions{
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
			if err := w.insertUsage(ctx, orgID, projectSlug, component, environment, "storage_gib_seconds", storageGiBSeconds, "GiB-s", now); err != nil {
				slog.Warn("insert storage usage", "err", err)
			}
		}

		if err := w.insertUsage(ctx, orgID, projectSlug, component, environment, "cpu_seconds", cpuSeconds, "cpu-s", now); err != nil {
			slog.Warn("insert cpu usage", "err", err)
		}
		if err := w.insertUsage(ctx, orgID, projectSlug, component, environment, "mem_gib_seconds", memGiBSeconds, "GiB-s", now); err != nil {
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
func (w *Worker) insertUsage(ctx context.Context, orgID, projectSlug, component, environment, resourceType string, quantity float64, unit string, at time.Time) error {
	var env any
	if environment != "" {
		env = environment
	}
	_, err := w.db.Exec(ctx, `
		INSERT INTO usage_events (org_id, project_id, component, environment, resource_type, quantity, unit, recorded_at)
		SELECT id, $2, $3, $4, $5, $6, $7, $8
		FROM organizations WHERE id = $1::uuid
	`, orgID, projectSlug, component, env, resourceType, quantity, unit, at)
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

	// Track per-(org, project, component) byte counts and flush every 60s. The org
	// UUID is taken from the project pod's `user-<orgID>` namespace so insertUsage
	// can attribute the flow to the right organization. Component is "workspace"
	// unless the flow's peer is the Gitea/Zot backend, in which case the bandwidth
	// is attributed to that platform service.
	type flowKey struct{ orgID, project, component string }
	type byteCounts struct{ ingress, egress int64 }
	counts := map[flowKey]*byteCounts{}

	flush := time.NewTicker(60 * time.Second)
	defer flush.Stop()

	go func() {
		for range flush.C {
			for key, bc := range counts {
				now := time.Now().UTC()
				if bc.ingress > 0 {
					if err := w.insertUsage(ctx, key.orgID, key.project, key.component, "", "net_ingress_bytes", float64(bc.ingress), "bytes", now); err != nil {
						slog.Warn("insert ingress usage", "err", err)
					}
				}
				if bc.egress > 0 {
					if err := w.insertUsage(ctx, key.orgID, key.project, key.component, "", "net_egress_bytes", float64(bc.egress), "bytes", now); err != nil {
						slog.Warn("insert egress usage", "err", err)
					}
				}
			}
			counts = map[flowKey]*byteCounts{}
		}
	}()

	var hubbleMissing bool
	for {
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

			// Identify the project endpoint (and its org) from pod labels and the
			// `user-<orgID>` namespace. Source being the project ⇒ egress; dest ⇒ ingress.
			srcOrg, srcProject := endpointOrgProject(flow.Source)
			dstOrg, dstProject := endpointOrgProject(flow.Destination)

			var key flowKey
			egress := false
			switch {
			case srcProject != "":
				// Project ⇒ peer: egress. Component reflects the peer (gitea/zot).
				key, egress = flowKey{srcOrg, srcProject, peerComponent(flow.Destination)}, true
			case dstProject != "":
				// Peer ⇒ project: ingress.
				key = flowKey{dstOrg, dstProject, peerComponent(flow.Source)}
			default:
				continue
			}
			if key.orgID == "" {
				continue
			}

			if _, ok := counts[key]; !ok {
				counts[key] = &byteCounts{}
			}
			if egress {
				counts[key].egress += 1500 // approximate MTU per flow record
			} else {
				counts[key].ingress += 1500
			}
		}
		_ = f.Close()
		time.Sleep(100 * time.Millisecond)
	}
}

// endpointOrgProject returns the org UUID (from the `user-<orgID>` namespace)
// and project slug for an endpoint, or empty strings if it isn't a project pod.
func endpointOrgProject(ep *HubbleEndpoint) (orgID, project string) {
	if ep == nil {
		return "", ""
	}
	project = extractProjectSlug(ep)
	if project != "" && strings.HasPrefix(ep.Namespace, "user-") {
		orgID = strings.TrimPrefix(ep.Namespace, "user-")
	}
	return orgID, project
}

// peerComponent classifies the non-project side of a flow so bandwidth to the
// platform's Gitea/Zot backends is attributed to those components. Anything else
// (project egress to the internet, between project pods) counts as "workspace".
func peerComponent(ep *HubbleEndpoint) string {
	if ep == nil || ep.Namespace != "enzarb-system" {
		return "workspace"
	}
	switch {
	case strings.HasPrefix(ep.PodName, "gitea"):
		return "gitea"
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

const gib = 1024 * 1024 * 1024

// orgIDBySlug loads a slug→UUID map for all organizations, so Gitea/Zot usage
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

// collectGiteaUsage polls the Gitea admin API for per-repo sizes and records them
// as gitea_storage_gib_seconds, attributed to the project whose slug matches the
// repo name within each org. Mirrors app/src/lib/gitea.ts auth.
func (w *Worker) collectGiteaUsage(ctx context.Context) error {
	if w.cfg.giteaURL == "" || w.cfg.giteaToken == "" {
		slog.Warn("gitea metering skipped: GITEA_URL or GITEA_ADMIN_TOKEN not set — git storage will not appear in billing")
		return nil
	}
	orgIDs, err := w.orgIDBySlug(ctx)
	if err != nil {
		return fmt.Errorf("org ids: %w", err)
	}
	now := time.Now().UTC()

	for slug, orgID := range orgIDs {
		page := 1
		for {
			var repos []struct {
				Name string `json:"name"`
				Size int64  `json:"size"` // KiB
			}
			path := fmt.Sprintf("/api/v1/orgs/%s/repos?limit=50&page=%d", url.PathEscape(slug), page)
			status, err := w.giteaGet(ctx, path, &repos)
			if err != nil {
				slog.Warn("gitea repos", "org", slug, "err", err)
				break
			}
			if status == http.StatusNotFound {
				break // org has no Gitea org mirror
			}
			for _, r := range repos {
				sizeGiB := float64(r.Size*1024) / gib
				if err := w.insertUsage(ctx, orgID, r.Name, "gitea", "", "gitea_storage_gib_seconds", sizeGiB*60.0, "GiB-s", now); err != nil {
					slog.Warn("insert gitea usage", "err", err)
				}
			}
			if len(repos) < 50 {
				break
			}
			page++
		}
	}
	return nil
}

func (w *Worker) giteaGet(ctx context.Context, path string, out any) (int, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, w.cfg.giteaURL+path, nil)
	if err != nil {
		return 0, err
	}
	req.Header.Set("Authorization", "token "+w.cfg.giteaToken)
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
		return resp.StatusCode, fmt.Errorf("gitea %d: %s", resp.StatusCode, body)
	}
	if err := json.NewDecoder(resp.Body).Decode(out); err != nil {
		return resp.StatusCode, err
	}
	return resp.StatusCode, nil
}

// collectZotUsage lists registry repositories and sums distinct blob sizes per
// repo (deduped by digest), recording zot_storage_gib_seconds. Repo paths follow
// the <orgSlug>/<image> convention, so the first segment maps to the org and the
// remainder is used as the project_id. Mirrors app/src/lib/zot.ts auth.
func (w *Worker) collectZotUsage(ctx context.Context) error {
	if w.cfg.registrySecret == "" {
		return nil // not configured; skip silently
	}
	orgIDs, err := w.orgIDBySlug(ctx)
	if err != nil {
		return fmt.Errorf("org ids: %w", err)
	}
	now := time.Now().UTC()

	var catalog struct {
		Repositories []string `json:"repositories"`
	}
	if _, err := w.zotGet(ctx, "/v2/_catalog", "registry:catalog:*", &catalog); err != nil {
		return fmt.Errorf("catalog: %w", err)
	}

	for _, repo := range catalog.Repositories {
		orgSlug, project, ok := strings.Cut(repo, "/")
		if !ok {
			continue
		}
		orgID, ok := orgIDs[orgSlug]
		if !ok {
			continue // repo not owned by a known org
		}
		sizeBytes, err := w.zotRepoSize(ctx, repo)
		if err != nil {
			slog.Warn("zot repo size", "repo", repo, "err", err)
			continue
		}
		sizeGiB := float64(sizeBytes) / gib
		if err := w.insertUsage(ctx, orgID, project, "zot", "", "zot_storage_gib_seconds", sizeGiB*60.0, "GiB-s", now); err != nil {
			slog.Warn("insert zot usage", "err", err)
		}
	}
	return nil
}

// zotRepoSize sums the size of every distinct blob (config + layers, deduped by
// digest across all tags) referenced by a repository's manifests.
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
		Digest string `json:"digest"`
		Size   int64  `json:"size"`
	}
	seen := map[string]int64{}
	for _, tag := range tags.Tags {
		var manifest struct {
			Config descriptor   `json:"config"`
			Layers []descriptor `json:"layers"`
		}
		if status, err := w.zotGet(ctx, "/v2/"+repo+"/manifests/"+tag, scope, &manifest); err != nil {
			slog.Warn("zot manifest", "repo", repo, "tag", tag, "err", err)
			continue
		} else if status == http.StatusNotFound {
			continue
		}
		if manifest.Config.Digest != "" {
			seen[manifest.Config.Digest] = manifest.Config.Size
		}
		for _, l := range manifest.Layers {
			seen[l.Digest] = l.Size
		}
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
