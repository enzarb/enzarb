package controller

import (
	"context"
	"fmt"
	"os"
	"reflect"
	"time"

	certmanagerv1 "github.com/cert-manager/cert-manager/pkg/apis/certmanager/v1"
	cmmeta "github.com/cert-manager/cert-manager/pkg/apis/meta/v1"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	apimeta "k8s.io/apimachinery/pkg/api/meta"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/intstr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/log"
	gatewayv1 "sigs.k8s.io/gateway-api/apis/v1"
	gatewayv1beta1 "sigs.k8s.io/gateway-api/apis/v1beta1"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
	"enzarb.dev/enzarb/operator/internal/gitea"
)

type ProjectReconciler struct {
	client.Client
	Scheme      *runtime.Scheme
	GiteaClient *gitea.Client
	Domain      string
}

func (r *ProjectReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&enzarbv1alpha1.Project{}).
		Owns(&appsv1.Deployment{}).
		Owns(&corev1.ServiceAccount{}).
		Owns(&corev1.PersistentVolumeClaim{}).
		Owns(&corev1.Service{}).
		Complete(r)
}

func (r *ProjectReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var project enzarbv1alpha1.Project
	if err := r.Get(ctx, req.NamespacedName, &project); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	orgNS := fmt.Sprintf("user-%s", project.Spec.OrgID)

	// Real (hard) deletion in progress: run cleanup of resources that aren't
	// garbage-collected by owner references (cluster-scoped CRB, cross-namespace
	// HTTPRoute/Certificate), then drop the finalizer.
	if !project.DeletionTimestamp.IsZero() {
		return r.reconcileProjectDelete(ctx, &project, orgNS)
	}

	// Ensure the cleanup finalizer is present before provisioning anything that
	// lives outside the project's namespace.
	if !controllerutil.ContainsFinalizer(&project, projectFinalizer) {
		controllerutil.AddFinalizer(&project, projectFinalizer)
		if err := r.Update(ctx, &project); err != nil {
			return ctrl.Result{}, fmt.Errorf("add finalizer: %w", err)
		}
		return ctrl.Result{Requeue: true}, nil
	}

	// Soft-deleted (retention window): keep data, scale the workspace to zero,
	// and hard-delete once the purge time passes.
	if purgeTime, ok := purgeAfter(&project); ok {
		return r.reconcileProjectRetention(ctx, &project, orgNS, purgeTime)
	}

	// The namespace is provisioned and owned by the Organization reconciler, not
	// here. If it doesn't exist yet, wait — never create it ourselves.
	ns := &corev1.Namespace{}
	if err := r.Get(ctx, types.NamespacedName{Name: orgNS}, ns); err != nil {
		if errors.IsNotFound(err) {
			logger.Info("waiting for org namespace", "namespace", orgNS)
			project.Status.Phase = "Pending"
			apimeta.SetStatusCondition(&project.Status.Conditions, metav1.Condition{
				Type:    "Ready",
				Status:  metav1.ConditionFalse,
				Reason:  "NamespaceMissing",
				Message: fmt.Sprintf("waiting for namespace %s to be provisioned", orgNS),
			})
			if err := r.Status().Update(ctx, &project); err != nil {
				return ctrl.Result{}, fmt.Errorf("update status: %w", err)
			}
			return ctrl.Result{RequeueAfter: defaultRequeue}, nil
		}
		return ctrl.Result{}, fmt.Errorf("get namespace: %w", err)
	}

	// Registry and Gitea paths are keyed by the human-readable org slug.
	orgSlug, err := r.orgSlug(ctx, project.Spec.OrgID)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("resolve org slug: %w", err)
	}

	saName := fmt.Sprintf("%s-sa", project.Spec.Slug)
	if err := r.ensureServiceAccount(ctx, orgNS, saName, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure service account: %w", err)
	}

	if err := r.ensureClusterRoleBinding(ctx, &project, orgNS, saName); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure cluster role binding: %w", err)
	}

	pvcName := fmt.Sprintf("%s-home", project.Spec.Slug)
	if err := r.ensurePVC(ctx, orgNS, pvcName, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure PVC: %w", err)
	}

	if err := r.ensureDeployment(ctx, orgNS, saName, pvcName, orgSlug, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deployment: %w", err)
	}

	if err := r.ensureService(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure service: %w", err)
	}

	if err := r.ensureGiteaRepo(ctx, orgSlug, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure gitea repo: %w", err)
	}

	if err := r.ensureReferenceGrant(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure referencegrant: %w", err)
	}

	if err := r.ensureHTTPRoute(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure httproute: %w", err)
	}

	if err := r.ensureCertificate(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure certificate: %w", err)
	}

	agentPath := agentPathFor(&project)
	project.Status.Phase = "Running"
	project.Status.ServiceAccountName = saName
	project.Status.AgentPath = agentPath
	if err := r.Status().Update(ctx, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("update status: %w", err)
	}

	logger.Info("project reconciled", "name", project.Name, "namespace", project.Namespace)
	return ctrl.Result{}, nil
}

// projectFinalizer guards cleanup of resources outside the project's namespace
// (cluster-scoped ClusterRoleBinding, enzarb-system HTTPRoute/Certificate) that
// owner-reference GC can't reach.
const projectFinalizer = "enzarb.io/project-cleanup"

// reconcileProjectRetention handles a soft-deleted project: scale its workspace
// to zero (retain the PVC/data), and hard-delete once the purge time arrives.
func (r *ProjectReconciler) reconcileProjectRetention(ctx context.Context, project *enzarbv1alpha1.Project, ns string, purgeTime time.Time) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	if !time.Now().Before(purgeTime) {
		logger.Info("purging soft-deleted project", "name", project.Name)
		if err := r.Delete(ctx, project); err != nil {
			return ctrl.Result{}, fmt.Errorf("purge project: %w", err)
		}
		return ctrl.Result{}, nil
	}

	// Within the retention window: scale the workspace down to stop compute.
	if err := r.scaleWorkspace(ctx, ns, project, 0); err != nil {
		return ctrl.Result{}, fmt.Errorf("scale down workspace: %w", err)
	}
	project.Status.Phase = "PendingDeletion"
	apimeta.SetStatusCondition(&project.Status.Conditions, metav1.Condition{
		Type:    "Ready",
		Status:  metav1.ConditionFalse,
		Reason:  "Retained",
		Message: fmt.Sprintf("soft-deleted; recoverable until %s", purgeTime.Format(time.RFC3339)),
	})
	if err := r.Status().Update(ctx, project); err != nil {
		return ctrl.Result{}, fmt.Errorf("update status: %w", err)
	}
	return ctrl.Result{RequeueAfter: time.Until(purgeTime)}, nil
}

// scaleWorkspace sets the workspace Deployment replica count, tolerating a
// not-yet-created deployment.
func (r *ProjectReconciler) scaleWorkspace(ctx context.Context, ns string, project *enzarbv1alpha1.Project, replicas int32) error {
	deployName := fmt.Sprintf("project-%s", project.Spec.Slug)
	deploy := &appsv1.Deployment{}
	if err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: deployName}, deploy); err != nil {
		if errors.IsNotFound(err) {
			return nil
		}
		return err
	}
	if deploy.Spec.Replicas != nil && *deploy.Spec.Replicas == replicas {
		return nil
	}
	deploy.Spec.Replicas = &replicas
	return r.Update(ctx, deploy)
}

// reconcileProjectDelete cleans up out-of-namespace resources on hard deletion,
// then removes the finalizer. In-namespace children (SA/PVC/Deployment/Service)
// are garbage-collected via their owner references.
func (r *ProjectReconciler) reconcileProjectDelete(ctx context.Context, project *enzarbv1alpha1.Project, ns string) (ctrl.Result, error) {
	logger := log.FromContext(ctx)
	if !controllerutil.ContainsFinalizer(project, projectFinalizer) {
		return ctrl.Result{}, nil
	}

	crbName := fmt.Sprintf("enzarb-%s-%s-deployer", project.Spec.OrgID, project.Spec.Slug)
	if err := r.deleteIfExists(ctx, &rbacv1.ClusterRoleBinding{
		ObjectMeta: metav1.ObjectMeta{Name: crbName},
	}); err != nil {
		return ctrl.Result{}, fmt.Errorf("delete cluster role binding: %w", err)
	}
	routeName := fmt.Sprintf("project-%s-agent", project.Spec.Slug)
	if err := r.deleteIfExists(ctx, &gatewayv1.HTTPRoute{
		ObjectMeta: metav1.ObjectMeta{Name: routeName, Namespace: "enzarb-system"},
	}); err != nil {
		return ctrl.Result{}, fmt.Errorf("delete httproute: %w", err)
	}
	certName := fmt.Sprintf("project-%s-tls", project.Spec.Slug)
	if err := r.deleteIfExists(ctx, &certmanagerv1.Certificate{
		ObjectMeta: metav1.ObjectMeta{Name: certName, Namespace: "enzarb-system"},
	}); err != nil {
		return ctrl.Result{}, fmt.Errorf("delete certificate: %w", err)
	}

	if err := r.cleanupGiteaRepo(ctx, project); err != nil {
		return ctrl.Result{}, fmt.Errorf("cleanup gitea repo: %w", err)
	}

	controllerutil.RemoveFinalizer(project, projectFinalizer)
	if err := r.Update(ctx, project); err != nil {
		return ctrl.Result{}, fmt.Errorf("remove finalizer: %w", err)
	}
	logger.Info("project deleted", "name", project.Name, "namespace", ns)
	return ctrl.Result{}, nil
}

func (r *ProjectReconciler) deleteIfExists(ctx context.Context, obj client.Object) error {
	if err := r.Delete(ctx, obj); err != nil && !errors.IsNotFound(err) {
		return err
	}
	return nil
}

func (r *ProjectReconciler) ensureServiceAccount(ctx context.Context, ns, name string, project *enzarbv1alpha1.Project) error {
	sa := &corev1.ServiceAccount{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, sa)
	if errors.IsNotFound(err) {
		sa = &corev1.ServiceAccount{
			ObjectMeta: metav1.ObjectMeta{
				Name:      name,
				Namespace: ns,
				Labels:    projectLabels(project),
			},
		}
		if err := controllerutil.SetControllerReference(project, sa, r.Scheme); err != nil {
			return err
		}
		return r.Create(ctx, sa)
	}
	return err
}

func (r *ProjectReconciler) ensureClusterRoleBinding(ctx context.Context, project *enzarbv1alpha1.Project, ns, saName string) error {
	bindingName := fmt.Sprintf("enzarb-%s-%s-deployer", project.Spec.OrgID, project.Spec.Slug)
	crb := &rbacv1.ClusterRoleBinding{}
	err := r.Get(ctx, types.NamespacedName{Name: bindingName}, crb)
	if errors.IsNotFound(err) {
		crb = &rbacv1.ClusterRoleBinding{
			ObjectMeta: metav1.ObjectMeta{
				Name:   bindingName,
				Labels: projectLabels(project),
			},
			RoleRef: rbacv1.RoleRef{
				APIGroup: "rbac.authorization.k8s.io",
				Kind:     "ClusterRole",
				Name:     "enzarb-deployer",
			},
			Subjects: []rbacv1.Subject{
				{Kind: "ServiceAccount", Name: saName, Namespace: ns},
			},
		}
		return r.Create(ctx, crb)
	}
	return err
}

func (r *ProjectReconciler) ensurePVC(ctx context.Context, ns, name string, project *enzarbv1alpha1.Project) error {
	pvc := &corev1.PersistentVolumeClaim{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, pvc)
	if errors.IsNotFound(err) {
		// If WORKSPACE_STORAGE_CLASS is unset, leave StorageClassName nil so the
		// cluster's default StorageClass is used (portable across clusters).
		var storageClassName *string
		if sc := os.Getenv("WORKSPACE_STORAGE_CLASS"); sc != "" {
			storageClassName = &sc
		}
		pvc = &corev1.PersistentVolumeClaim{
			ObjectMeta: metav1.ObjectMeta{
				Name:      name,
				Namespace: ns,
				Labels:    projectLabels(project),
			},
			Spec: corev1.PersistentVolumeClaimSpec{
				AccessModes:      []corev1.PersistentVolumeAccessMode{corev1.ReadWriteOnce},
				StorageClassName: storageClassName,
				Resources: corev1.VolumeResourceRequirements{
					Requests: corev1.ResourceList{
						corev1.ResourceStorage: project.Spec.Storage.Size,
					},
				},
			},
		}
		if err := controllerutil.SetControllerReference(project, pvc, r.Scheme); err != nil {
			return err
		}
		return r.Create(ctx, pvc)
	}
	if err != nil {
		return err
	}
	// Expand PVC if spec changed
	currentSize := pvc.Spec.Resources.Requests[corev1.ResourceStorage]
	if project.Spec.Storage.Size.Cmp(currentSize) > 0 {
		pvc.Spec.Resources.Requests[corev1.ResourceStorage] = project.Spec.Storage.Size
		return r.Update(ctx, pvc)
	}
	return nil
}

func (r *ProjectReconciler) ensureDeployment(ctx context.Context, ns, saName, pvcName, orgSlug string, project *enzarbv1alpha1.Project) error {
	deployName := fmt.Sprintf("project-%s", project.Spec.Slug)
	deploy := &appsv1.Deployment{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: deployName}, deploy)
	desired := r.buildDeployment(ns, deployName, saName, pvcName, orgSlug, project)
	if err := controllerutil.SetControllerReference(project, desired, r.Scheme); err != nil {
		return err
	}
	if errors.IsNotFound(err) {
		return r.Create(ctx, desired)
	}
	if err != nil {
		return err
	}
	// Update deployment to apply spec changes
	deploy.Spec = desired.Spec
	return r.Update(ctx, deploy)
}

func (r *ProjectReconciler) buildDeployment(ns, name, saName, pvcName, orgSlug string, project *enzarbv1alpha1.Project) *appsv1.Deployment {
	labels := projectLabels(project)
	replicas := int32(1)

	toolsJSON := toolsToJSON(project.Spec.Tools)

	// Default resource requests if not specified
	cpuReq := resource.MustParse("500m")
	memReq := resource.MustParse("512Mi")
	cpuLim := resource.MustParse("2")
	memLim := resource.MustParse("2Gi")
	if !project.Spec.Resources.Requests.Cpu().IsZero() {
		cpuReq = *project.Spec.Resources.Requests.Cpu()
	}
	if !project.Spec.Resources.Requests.Memory().IsZero() {
		memReq = *project.Spec.Resources.Requests.Memory()
	}
	if !project.Spec.Resources.Limits.Cpu().IsZero() {
		cpuLim = *project.Spec.Resources.Limits.Cpu()
	}
	if !project.Spec.Resources.Limits.Memory().IsZero() {
		memLim = *project.Spec.Resources.Limits.Memory()
	}

	return &appsv1.Deployment{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
			Labels:    labels,
		},
		Spec: appsv1.DeploymentSpec{
			Replicas: &replicas,
			Selector: &metav1.LabelSelector{MatchLabels: labels},
			// Recreate (not RollingUpdate): the home PVC is ReadWriteOnce and the
			// workspace is a single replica, so rollouts must tear down the old
			// pod before starting the new one (avoids volume multi-attach and
			// single-node CPU deadlock during image bumps).
			Strategy: appsv1.DeploymentStrategy{Type: appsv1.RecreateDeploymentStrategyType},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{Labels: labels},
				Spec: corev1.PodSpec{
					ServiceAccountName: saName,
					SecurityContext: &corev1.PodSecurityContext{
						RunAsNonRoot: boolPtr(true),
						RunAsUser:    int64Ptr(1000),
						RunAsGroup:   int64Ptr(1000),
						// Chown mounted volumes to GID 1000 so the non-root agent
						// can write to a freshly-provisioned home PVC (block volumes
						// are owned by root until fsGroup is applied).
						FSGroup: int64Ptr(1000),
						SeccompProfile: &corev1.SeccompProfile{
							Type: corev1.SeccompProfileTypeRuntimeDefault,
						},
					},
					Containers: []corev1.Container{
						{
							Name:  "workspace",
							Image: workspaceImage(),
							Env: []corev1.EnvVar{
								{Name: "ENZARB_PROJECT_ID", Value: string(project.UID)},
								{Name: "ENZARB_PROJECT_SLUG", Value: project.Spec.Slug},
								{Name: "ENZARB_ORG_ID", Value: project.Spec.OrgID},
								{Name: "ENZARB_TOOLS", Value: toolsJSON},
								// Preconfigured registry + git coordinates (GHCR-style). The
								// workspace's credential helpers auth to these automatically; the
								// project may only push/pull within its own <orgSlug>/<slug> prefix.
								{Name: "ENZARB_REGISTRY", Value: fmt.Sprintf("registry.%s", r.Domain)},
								{Name: "ENZARB_IMAGE", Value: fmt.Sprintf("registry.%s/%s/%s", r.Domain, orgSlug, project.Spec.Slug)},
								{Name: "ENZARB_GIT_REMOTE", Value: fmt.Sprintf("https://gitea.%s/%s/%s.git", r.Domain, orgSlug, project.Spec.Slug)},
								// buildkitd sidecar speaks the BuildKit gRPC API, not the
								// Docker daemon API — clients reach it via BUILDKIT_HOST.
								{Name: "BUILDKIT_HOST", Value: "tcp://localhost:1234"},
								{Name: "HOME", Value: "/home/user"},
							},
							Ports: []corev1.ContainerPort{
								{Name: "agent-external", ContainerPort: 8080},
								{Name: "agent-internal", ContainerPort: 9090},
							},
							Resources: corev1.ResourceRequirements{
								Requests: corev1.ResourceList{
									corev1.ResourceCPU:    cpuReq,
									corev1.ResourceMemory: memReq,
								},
								Limits: corev1.ResourceList{
									corev1.ResourceCPU:    cpuLim,
									corev1.ResourceMemory: memLim,
								},
							},
							VolumeMounts: []corev1.VolumeMount{
								{Name: "home", MountPath: "/home/user"},
								{Name: "tmp", MountPath: "/tmp"},
								{Name: "registry-token", MountPath: "/var/run/secrets/enzarb/registry"},
								{Name: "gitea-token", MountPath: "/var/run/secrets/enzarb/gitea"},
							},
							SecurityContext: &corev1.SecurityContext{
								AllowPrivilegeEscalation: boolPtr(false),
								ReadOnlyRootFilesystem:   boolPtr(true),
								Capabilities: &corev1.Capabilities{
									Drop: []corev1.Capability{"ALL"},
								},
							},
							LivenessProbe: &corev1.Probe{
								ProbeHandler: corev1.ProbeHandler{
									HTTPGet: &corev1.HTTPGetAction{
										Path: "/health",
										Port: intstr.FromInt32(9090),
									},
								},
							},
							ReadinessProbe: &corev1.Probe{
								ProbeHandler: corev1.ProbeHandler{
									HTTPGet: &corev1.HTTPGetAction{
										Path: "/health",
										Port: intstr.FromInt32(9090),
									},
								},
							},
						},
						{
							Name:  "buildkitd",
							Image: "moby/buildkit:rootless",
							Args: []string{
								"--addr", "tcp://0.0.0.0:1234",
								// Required to run rootless buildkitd in an unprivileged container.
								"--oci-worker-no-process-sandbox",
							},
							Ports: []corev1.ContainerPort{{Name: "buildkitd", ContainerPort: 1234}},
							SecurityContext: &corev1.SecurityContext{
								RunAsUser:  int64Ptr(1000),
								RunAsGroup: int64Ptr(1000),
								// Rootless buildkit needs seccomp + AppArmor unconfined to
								// set up its user-namespace worker without privileges.
								SeccompProfile: &corev1.SeccompProfile{
									Type: corev1.SeccompProfileTypeUnconfined,
								},
								AppArmorProfile: &corev1.AppArmorProfile{
									Type: corev1.AppArmorProfileTypeUnconfined,
								},
							},
						},
					},
					Volumes: []corev1.Volume{
						{
							Name: "home",
							VolumeSource: corev1.VolumeSource{
								PersistentVolumeClaim: &corev1.PersistentVolumeClaimVolumeSource{
									ClaimName: pvcName,
								},
							},
						},
						{
							Name: "tmp",
							VolumeSource: corev1.VolumeSource{
								EmptyDir: &corev1.EmptyDirVolumeSource{
									Medium: corev1.StorageMediumMemory,
								},
							},
						},
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
			},
		},
	}
}

func (r *ProjectReconciler) ensureService(ctx context.Context, ns string, project *enzarbv1alpha1.Project) error {
	svcName := fmt.Sprintf("project-%s", project.Spec.Slug)
	svc := &corev1.Service{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: svcName}, svc)
	if errors.IsNotFound(err) {
		svc = &corev1.Service{
			ObjectMeta: metav1.ObjectMeta{
				Name:      svcName,
				Namespace: ns,
				Labels:    projectLabels(project),
			},
			Spec: corev1.ServiceSpec{
				Selector: projectLabels(project),
				Ports: []corev1.ServicePort{
					{Name: "agent-external", Port: 8080},
					{Name: "agent-internal", Port: 9090},
				},
			},
		}
		if err := controllerutil.SetControllerReference(project, svc, r.Scheme); err != nil {
			return err
		}
		return r.Create(ctx, svc)
	}
	return err
}

func (r *ProjectReconciler) ensureGiteaRepo(ctx context.Context, orgSlug string, project *enzarbv1alpha1.Project) error {
	if r.GiteaClient == nil {
		return nil
	}
	// Keyed by the human-readable org slug, matching the registry prefix and the
	// X-Gitea-User identity authd asserts.
	if err := r.GiteaClient.EnsureOrg(orgSlug); err != nil {
		return fmt.Errorf("ensure gitea org: %w", err)
	}
	if _, err := r.GiteaClient.CreateRepo(orgSlug, gitea.CreateRepoRequest{
		Name:          project.Spec.Slug,
		Description:   project.Spec.DisplayName,
		Private:       true,
		AutoInit:      true,
		DefaultBranch: "main",
	}); err != nil {
		return err
	}

	// Provision the per-project Gitea identity that authd asserts via
	// reverse-proxy auth, and grant it write to only this repo. This is what
	// keeps git private-by-default and isolated per project, mirroring the
	// registry's <orgSlug>/<slug> scoping.
	// Gitea usernames disallow consecutive special chars (so no "--"); slugs are
	// [a-z0-9-] and never contain "_", so an underscore is a safe, unambiguous
	// separator. Must match the X-Gitea-User authd sets.
	user := fmt.Sprintf("%s_%s", orgSlug, project.Spec.Slug)
	email := fmt.Sprintf("%s@workspaces.%s", user, r.Domain)
	if err := r.GiteaClient.EnsureUser(user, email); err != nil {
		return fmt.Errorf("ensure gitea user: %w", err)
	}
	if err := r.GiteaClient.AddCollaborator(orgSlug, project.Spec.Slug, user, "write"); err != nil {
		return fmt.Errorf("add gitea collaborator: %w", err)
	}
	return nil
}

// cleanupGiteaRepo deletes the project's Gitea repo and per-project user on hard
// deletion, mirroring ensureGiteaRepo. The org and its other repos are left
// intact. Best-effort and idempotent: the Gitea client tolerates already-gone
// resources so finalizer removal isn't blocked by a partially-purged state.
func (r *ProjectReconciler) cleanupGiteaRepo(ctx context.Context, project *enzarbv1alpha1.Project) error {
	if r.GiteaClient == nil {
		return nil
	}
	orgSlug, err := r.orgSlug(ctx, project.Spec.OrgID)
	if err != nil {
		if errors.IsNotFound(err) {
			// Org already gone (e.g. org-level purge); nothing scoped to clean up.
			return nil
		}
		return fmt.Errorf("resolve org slug: %w", err)
	}
	if err := r.GiteaClient.DeleteRepo(orgSlug, project.Spec.Slug); err != nil {
		return err
	}
	// Delete the user after its repo so Gitea doesn't refuse on owned-repo checks.
	user := fmt.Sprintf("%s_%s", orgSlug, project.Spec.Slug)
	return r.GiteaClient.DeleteUser(user)
}

// orgSlug resolves a project's org id to the org's human-readable slug via the
// cluster-scoped Organization CR (its name is the org id).
func (r *ProjectReconciler) orgSlug(ctx context.Context, orgID string) (string, error) {
	var org enzarbv1alpha1.Organization
	if err := r.Get(ctx, types.NamespacedName{Name: orgID}, &org); err != nil {
		return "", err
	}
	if org.Spec.Slug == "" {
		return "", fmt.Errorf("organization %s has no slug", orgID)
	}
	return org.Spec.Slug, nil
}

// ensureReferenceGrant permits HTTPRoutes in enzarb-system (where the shared
// gateway lives) to reference agent Services in the project's namespace. Without
// it, cross-namespace backend refs are rejected (ResolvedRefs=RefNotPermitted).
// One grant per namespace covers every project's agent Service in that org.
func (r *ProjectReconciler) ensureReferenceGrant(ctx context.Context, ns string, project *enzarbv1alpha1.Project) error {
	grant := &gatewayv1beta1.ReferenceGrant{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: "agent-route-grant"}, grant)
	if errors.IsNotFound(err) {
		grant = &gatewayv1beta1.ReferenceGrant{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "agent-route-grant",
				Namespace: ns,
			},
			Spec: gatewayv1beta1.ReferenceGrantSpec{
				From: []gatewayv1beta1.ReferenceGrantFrom{{
					Group:     "gateway.networking.k8s.io",
					Kind:      "HTTPRoute",
					Namespace: "enzarb-system",
				}},
				To: []gatewayv1beta1.ReferenceGrantTo{{
					Group: "",
					Kind:  "Service",
				}},
			},
		}
		return r.Create(ctx, grant)
	}
	return err
}

// agentPathFor is the single source of truth for a project's agent route prefix.
// The UID keeps it globally unique across orgs that share the same hostname.
func agentPathFor(project *enzarbv1alpha1.Project) string {
	return fmt.Sprintf("/agent/%s", string(project.UID))
}

func (r *ProjectReconciler) ensureHTTPRoute(ctx context.Context, ns string, project *enzarbv1alpha1.Project) error {
	routeName := fmt.Sprintf("project-%s-agent", project.Spec.Slug)
	hostname := gatewayv1.Hostname(r.Domain)
	pathPrefix := agentPathFor(project)
	pathType := gatewayv1.PathMatchPathPrefix
	svcName := gatewayv1.ObjectName(fmt.Sprintf("project-%s", project.Spec.Slug))
	port := gatewayv1.PortNumber(8080)
	backendNS := gatewayv1.Namespace(ns)
	gatewayNS := gatewayv1.Namespace("enzarb-system")
	httpsSection := gatewayv1.SectionName("https")
	gatewayKind := gatewayv1.Kind("Gateway")
	rewriteRoot := "/"

	desiredSpec := gatewayv1.HTTPRouteSpec{
		CommonRouteSpec: gatewayv1.CommonRouteSpec{
			ParentRefs: []gatewayv1.ParentReference{{
				Name:        "enzarb",
				Namespace:   &gatewayNS,
				Kind:        &gatewayKind,
				SectionName: &httpsSection,
			}},
		},
		Hostnames: []gatewayv1.Hostname{hostname},
		Rules: []gatewayv1.HTTPRouteRule{{
			Matches: []gatewayv1.HTTPRouteMatch{{
				Path: &gatewayv1.HTTPPathMatch{
					Type:  &pathType,
					Value: &pathPrefix,
				},
			}},
			// Strip the `/agent/<uid>` prefix so the agent sees its own routes
			// (`/processes`, `/files`, …) rather than the public path.
			Filters: []gatewayv1.HTTPRouteFilter{{
				Type: gatewayv1.HTTPRouteFilterURLRewrite,
				URLRewrite: &gatewayv1.HTTPURLRewriteFilter{
					Path: &gatewayv1.HTTPPathModifier{
						Type:               gatewayv1.PrefixMatchHTTPPathModifier,
						ReplacePrefixMatch: &rewriteRoot,
					},
				},
			}},
			BackendRefs: []gatewayv1.HTTPBackendRef{{
				BackendRef: gatewayv1.BackendRef{
					BackendObjectReference: gatewayv1.BackendObjectReference{
						Name:      svcName,
						Namespace: &backendNS,
						Port:      &port,
					},
				},
			}},
		}},
	}

	route := &gatewayv1.HTTPRoute{}
	err := r.Get(ctx, types.NamespacedName{Namespace: "enzarb-system", Name: routeName}, route)
	if errors.IsNotFound(err) {
		route = &gatewayv1.HTTPRoute{
			ObjectMeta: metav1.ObjectMeta{
				Name:      routeName,
				Namespace: "enzarb-system",
				Labels:    projectLabels(project),
			},
			Spec: desiredSpec,
		}
		return r.Create(ctx, route)
	}
	if err != nil {
		return err
	}

	// Reconcile drift on existing routes (e.g. older versions wrote a bad spec).
	if !reflect.DeepEqual(route.Spec, desiredSpec) {
		route.Spec = desiredSpec
		return r.Update(ctx, route)
	}
	return nil
}

func (r *ProjectReconciler) ensureCertificate(ctx context.Context, ns string, project *enzarbv1alpha1.Project) error {
	certName := fmt.Sprintf("project-%s-tls", project.Spec.Slug)
	cert := &certmanagerv1.Certificate{}
	err := r.Get(ctx, types.NamespacedName{Namespace: "enzarb-system", Name: certName}, cert)
	if errors.IsNotFound(err) {
		cert = &certmanagerv1.Certificate{
			ObjectMeta: metav1.ObjectMeta{
				Name:      certName,
				Namespace: "enzarb-system",
				Labels:    projectLabels(project),
			},
			Spec: certmanagerv1.CertificateSpec{
				SecretName: fmt.Sprintf("project-%s-tls", project.Spec.Slug),
				DNSNames:   []string{r.Domain},
				IssuerRef: cmmeta.IssuerReference{
					Name:  "letsencrypt-prod",
					Kind:  "ClusterIssuer",
					Group: "cert-manager.io",
				},
			},
		}
		return r.Create(ctx, cert)
	}
	return err
}

func projectLabels(p *enzarbv1alpha1.Project) map[string]string {
	return map[string]string{
		"app.kubernetes.io/managed-by": "enzarb-operator",
		"enzarb.io/project":            p.Spec.Slug,
		"enzarb.io/org":                p.Spec.OrgID,
	}
}

// workspaceImage returns the workspace container image, pinned via the
// WORKSPACE_IMAGE env (set from the Helm chart's workspace.image values).
// Falls back to a floating tag for local/dev when unset.
func workspaceImage() string {
	if img := os.Getenv("WORKSPACE_IMAGE"); img != "" {
		return img
	}
	return "ghcr.io/enzarb/workspace:latest"
}

func int64Ptr(i int64) *int64 { return &i }
func boolPtr(b bool) *bool    { return &b }

func toolsToJSON(tools []enzarbv1alpha1.ProjectTool) string {
	if len(tools) == 0 {
		return "[]"
	}
	b := `[`
	for i, t := range tools {
		if i > 0 {
			b += ","
		}
		b += fmt.Sprintf(`{"name":%q,"version":%q}`, t.Name, t.Version)
	}
	return b + `]`
}
