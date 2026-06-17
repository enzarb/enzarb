package controller

import (
	"context"
	"fmt"
	"os"

	certmanagerv1 "github.com/cert-manager/cert-manager/pkg/apis/certmanager/v1"
	cmmeta "github.com/cert-manager/cert-manager/pkg/apis/meta/v1"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/intstr"
	gatewayv1 "sigs.k8s.io/gateway-api/apis/v1"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"

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

	if err := r.ensureNamespace(ctx, orgNS); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure namespace: %w", err)
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

	if err := r.ensureDeployment(ctx, orgNS, saName, pvcName, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deployment: %w", err)
	}

	if err := r.ensureService(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure service: %w", err)
	}

	if err := r.ensureGiteaRepo(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure gitea repo: %w", err)
	}

	if err := r.ensureHTTPRoute(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure httproute: %w", err)
	}

	if err := r.ensureCertificate(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure certificate: %w", err)
	}

	agentPath := fmt.Sprintf("/agent/%s", project.Name)
	project.Status.Phase = "Running"
	project.Status.ServiceAccountName = saName
	project.Status.AgentPath = agentPath
	if err := r.Status().Update(ctx, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("update status: %w", err)
	}

	logger.Info("project reconciled", "name", project.Name, "namespace", project.Namespace)
	return ctrl.Result{}, nil
}

func (r *ProjectReconciler) ensureNamespace(ctx context.Context, name string) error {
	ns := &corev1.Namespace{}
	err := r.Get(ctx, types.NamespacedName{Name: name}, ns)
	if errors.IsNotFound(err) {
		return r.Create(ctx, &corev1.Namespace{
			ObjectMeta: metav1.ObjectMeta{Name: name},
		})
	}
	return err
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
		storageClass := "standard"
		pvc = &corev1.PersistentVolumeClaim{
			ObjectMeta: metav1.ObjectMeta{
				Name:      name,
				Namespace: ns,
				Labels:    projectLabels(project),
			},
			Spec: corev1.PersistentVolumeClaimSpec{
				AccessModes:      []corev1.PersistentVolumeAccessMode{corev1.ReadWriteOnce},
				StorageClassName: &storageClass,
				Resources: corev1.VolumeResourceRequirements{
					Requests: corev1.ResourceList{
						corev1.ResourceStorage: project.Spec.Storage.Size,
					},
				},
			},
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

func (r *ProjectReconciler) ensureDeployment(ctx context.Context, ns, saName, pvcName string, project *enzarbv1alpha1.Project) error {
	deployName := fmt.Sprintf("project-%s", project.Spec.Slug)
	deploy := &appsv1.Deployment{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: deployName}, deploy)
	desired := r.buildDeployment(ns, deployName, saName, pvcName, project)
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

func (r *ProjectReconciler) buildDeployment(ns, name, saName, pvcName string, project *enzarbv1alpha1.Project) *appsv1.Deployment {
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
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{Labels: labels},
				Spec: corev1.PodSpec{
					ServiceAccountName: saName,
					Containers: []corev1.Container{
						{
							Name:  "workspace",
							Image: "ghcr.io/enzarb/workspace:latest",
							Env: []corev1.EnvVar{
								{Name: "ENZARB_PROJECT_ID", Value: string(project.UID)},
								{Name: "ENZARB_PROJECT_SLUG", Value: project.Spec.Slug},
								{Name: "ENZARB_ORG_ID", Value: project.Spec.OrgID},
								{Name: "ENZARB_TOOLS", Value: toolsJSON},
								{Name: "DOCKER_HOST", Value: "tcp://localhost:1234"},
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
								{Name: "registry-token", MountPath: "/var/run/secrets/enzarb/registry"},
								{Name: "gitea-token", MountPath: "/var/run/secrets/enzarb/gitea"},
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
							Image: "moby/buildkitd:rootless",
							Args:  []string{"--addr", "tcp://0.0.0.0:1234"},
							Ports: []corev1.ContainerPort{{Name: "buildkitd", ContainerPort: 1234}},
							SecurityContext: &corev1.SecurityContext{
								SeccompProfile: &corev1.SeccompProfile{
									Type: corev1.SeccompProfileTypeUnconfined,
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
		return r.Create(ctx, svc)
	}
	return err
}

func (r *ProjectReconciler) ensureGiteaRepo(ctx context.Context, ns string, project *enzarbv1alpha1.Project) error {
	if r.GiteaClient == nil {
		return nil
	}
	orgSlug := project.Spec.OrgID
	if err := r.GiteaClient.EnsureOrg(orgSlug); err != nil {
		return fmt.Errorf("ensure gitea org: %w", err)
	}
	_, err := r.GiteaClient.CreateRepo(orgSlug, gitea.CreateRepoRequest{
		Name:          project.Spec.Slug,
		Description:   project.Spec.DisplayName,
		Private:       true,
		AutoInit:      true,
		DefaultBranch: "main",
	})
	return err
}

func (r *ProjectReconciler) ensureHTTPRoute(ctx context.Context, ns string, project *enzarbv1alpha1.Project) error {
	routeName := fmt.Sprintf("project-%s-agent", project.Spec.Slug)
	hostname := gatewayv1.Hostname(r.Domain)
	pathPrefix := fmt.Sprintf("/agent/%s", string(project.UID))
	pathType := gatewayv1.PathMatchPathPrefix
	svcName := gatewayv1.ObjectName(fmt.Sprintf("project-%s", project.Spec.Slug))
	port := gatewayv1.PortNumber(8080)
	ns8080 := gatewayv1.Namespace(ns)

	route := &gatewayv1.HTTPRoute{}
	err := r.Get(ctx, types.NamespacedName{Namespace: "enzarb-system", Name: routeName}, route)
	if errors.IsNotFound(err) {
		route = &gatewayv1.HTTPRoute{
			ObjectMeta: metav1.ObjectMeta{
				Name:      routeName,
				Namespace: "enzarb-system",
				Labels:    projectLabels(project),
			},
			Spec: gatewayv1.HTTPRouteSpec{
				CommonRouteSpec: gatewayv1.CommonRouteSpec{
					ParentRefs: []gatewayv1.ParentReference{{
						Name: "envoy",
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
					BackendRefs: []gatewayv1.HTTPBackendRef{{
						BackendRef: gatewayv1.BackendRef{
							BackendObjectReference: gatewayv1.BackendObjectReference{
								Name:      svcName,
								Namespace: &ns8080,
								Port:      &port,
							},
						},
					}},
				}},
			},
		}
		return r.Create(ctx, route)
	}
	return err
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
				IssuerRef: cmmeta.ObjectReference{
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

// domainFromEnv reads ENZARB_DOMAIN env var, defaulting to "enzarb.dev".
func domainFromEnv() string {
	if d := os.Getenv("ENZARB_DOMAIN"); d != "" {
		return d
	}
	return "enzarb.dev"
}

func projectLabels(p *enzarbv1alpha1.Project) map[string]string {
	return map[string]string{
		"app.kubernetes.io/managed-by": "enzarb-operator",
		"enzarb.io/project":            p.Spec.Slug,
		"enzarb.io/org":                p.Spec.OrgID,
	}
}

func int64Ptr(i int64) *int64 { return &i }

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
