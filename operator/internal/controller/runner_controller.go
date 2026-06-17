package controller

import (
	"context"
	"fmt"
	"net/http"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/predicate"
)

// RunnerReconciler handles ephemeral act_runner pods for Gitea Actions jobs.
// It receives Gitea webhook events via HTTP and creates/cleans up runner pods.
type RunnerReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

func (r *RunnerReconciler) SetupWithManager(mgr ctrl.Manager) error {
	// Watch pods with runner label for cleanup on completion
	return ctrl.NewControllerManagedBy(mgr).
		For(&corev1.Pod{}).
		WithEventFilter(runnerPodFilter{}).
		Complete(r)
}

func (r *RunnerReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var pod corev1.Pod
	if err := r.Get(ctx, req.NamespacedName, &pod); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	if pod.Labels["enzarb.io/runner"] != "true" {
		return ctrl.Result{}, nil
	}

	// Delete completed/failed runner pods
	phase := pod.Status.Phase
	if phase == corev1.PodSucceeded || phase == corev1.PodFailed {
		logger.Info("cleaning up completed runner pod", "pod", pod.Name, "phase", phase)
		return ctrl.Result{}, client.IgnoreNotFound(r.Delete(ctx, &pod))
	}

	return ctrl.Result{}, nil
}

// HandleGiteaWebhook handles incoming Gitea Actions webhook events.
// Called by the operator's HTTP server when a job is queued.
func (r *RunnerReconciler) HandleGiteaWebhook(w http.ResponseWriter, req *http.Request) {
	ctx := req.Context()
	logger := log.FromContext(ctx)

	// TODO: parse Gitea webhook payload to extract org, project, job ID
	// For now, acknowledge receipt
	logger.Info("received Gitea webhook")
	w.WriteHeader(http.StatusOK)
}

// SpawnRunnerPod creates an ephemeral act_runner pod for a Gitea Actions job.
func (r *RunnerReconciler) SpawnRunnerPod(ctx context.Context, orgID, projectSlug, saName, orgNS, jobID, giteaURL, registrationToken string) error {
	podName := fmt.Sprintf("runner-%s-%s", projectSlug, jobID[:8])

	existing := &corev1.Pod{}
	err := r.Get(ctx, types.NamespacedName{Namespace: orgNS, Name: podName}, existing)
	if err == nil {
		return nil // already exists
	}
	if !errors.IsNotFound(err) {
		return err
	}

	pod := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name:      podName,
			Namespace: orgNS,
			Labels: map[string]string{
				"app.kubernetes.io/managed-by": "enzarb-operator",
				"enzarb.io/runner":             "true",
				"enzarb.io/project":            projectSlug,
				"enzarb.io/org":                orgID,
				"enzarb.io/job-id":             jobID,
			},
		},
		Spec: corev1.PodSpec{
			ServiceAccountName: saName,
			RestartPolicy:      corev1.RestartPolicyNever,
			Containers: []corev1.Container{
				{
					Name:  "runner",
					Image: "gitea/act_runner:latest",
					Env: []corev1.EnvVar{
						{Name: "GITEA_INSTANCE_URL", Value: giteaURL},
						{Name: "GITEA_RUNNER_REGISTRATION_TOKEN", Value: registrationToken},
						{Name: "GITEA_RUNNER_NAME", Value: podName},
					},
					VolumeMounts: []corev1.VolumeMount{
						{Name: "registry-token", MountPath: "/var/run/secrets/enzarb/registry"},
						{Name: "gitea-token", MountPath: "/var/run/secrets/enzarb/gitea"},
					},
				},
			},
			Volumes: []corev1.Volume{
				{
					Name: "registry-token",
					VolumeSource: corev1.VolumeSource{
						Projected: &corev1.ProjectedVolumeSource{
							Sources: []corev1.VolumeProjection{
								{
									ServiceAccountToken: &corev1.ServiceAccountTokenProjection{
										Audience:          "registry.enzarb.dev",
										ExpirationSeconds: int64Ptr(3600),
										Path:              "token",
									},
								},
							},
						},
					},
				},
				{
					Name: "gitea-token",
					VolumeSource: corev1.VolumeSource{
						Projected: &corev1.ProjectedVolumeSource{
							Sources: []corev1.VolumeProjection{
								{
									ServiceAccountToken: &corev1.ServiceAccountTokenProjection{
										Audience:          "gitea.enzarb.dev",
										ExpirationSeconds: int64Ptr(3600),
										Path:              "token",
									},
								},
							},
						},
					},
				},
			},
		},
	}

	return r.Create(ctx, pod)
}

// runnerPodFilter filters reconcile events to only runner-labeled pods.
type runnerPodFilter struct {
	predicate.Funcs
}

func (runnerPodFilter) Create(_ event.CreateEvent) bool  { return false }
func (runnerPodFilter) Delete(_ event.DeleteEvent) bool  { return false }
func (runnerPodFilter) Generic(_ event.GenericEvent) bool { return false }
func (runnerPodFilter) Update(e event.UpdateEvent) bool {
	pod, ok := e.ObjectNew.(*corev1.Pod)
	if !ok {
		return false
	}
	return pod.Labels["enzarb.io/runner"] == "true"
}
