package controller

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"

	certmanagerv1 "github.com/cert-manager/cert-manager/pkg/apis/certmanager/v1"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	networkingv1 "k8s.io/api/networking/v1"
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
	"sigs.k8s.io/controller-runtime/pkg/handler"
	"sigs.k8s.io/controller-runtime/pkg/log"
	gatewayv1 "sigs.k8s.io/gateway-api/apis/v1"
	gatewayv1beta1 "sigs.k8s.io/gateway-api/apis/v1beta1"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
)

type ProjectReconciler struct {
	client.Client
	Scheme *runtime.Scheme
	Domain string
	// APIReader bypasses the manager's informer cache — see capsule.go /
	// OrganizationReconciler.APIReader for why capsule lookups need it.
	APIReader client.Reader
}

func (r *ProjectReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&enzarbv1alpha1.Project{}).
		Owns(&appsv1.Deployment{}).
		Owns(&corev1.ServiceAccount{}).
		Owns(&corev1.PersistentVolumeClaim{}).
		Owns(&corev1.Service{}).
		Owns(&corev1.ConfigMap{}).
		Watches(&enzarbv1alpha1.Environment{}, handler.EnqueueRequestsFromMapFunc(r.envToProject)).
		Complete(r)
}

// envToProject maps an Environment event to the owning Project so the project
// reconciler re-runs whenever an Environment's status.namespace changes.
func (r *ProjectReconciler) envToProject(ctx context.Context, obj client.Object) []ctrl.Request {
	env, ok := obj.(*enzarbv1alpha1.Environment)
	if !ok {
		return nil
	}
	projectName := env.Spec.ProjectRef.Name
	if projectName == "" {
		return nil
	}
	return []ctrl.Request{{NamespacedName: types.NamespacedName{
		Namespace: env.Namespace,
		Name:      projectName,
	}}}
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

	// Registry paths are keyed by the human-readable org slug.
	orgSlug, err := r.orgSlug(ctx, project.Spec.OrgID)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("resolve org slug: %w", err)
	}

	saName := fmt.Sprintf("%s-sa", project.Spec.Slug)
	if err := r.ensureServiceAccount(ctx, orgNS, saName, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure service account: %w", err)
	}

	// Grant the workspace SA `get` on its environments' deploy namespaces.
	envNamespaces, err := r.projectEnvNamespaces(ctx, orgNS, &project)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("list environment namespaces: %w", err)
	}
	if err := r.ensureEnvNamespaceRBAC(ctx, orgNS, saName, &project, envNamespaces); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure env namespace rbac: %w", err)
	}

	if err := r.ensureCapsuleTenant(ctx, orgNS, saName, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure capsule tenant: %w", err)
	}

	if err := r.ensureWorkspaceKubeconfig(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure workspace kubeconfig: %w", err)
	}

	// Prune any stale ClusterRoleBindings left over from before the migration
	// to namespace-scoped RoleBindings. ClusterRoleBindings are cluster-scoped
	// so pass "" as namespace to pruneUnmanaged. The listing is cluster-wide
	// across all projects, so only consider bindings attributed to this
	// project (or legacy ones with no project label) — otherwise each
	// project's reconcile would delete every other project's env-ns binding.
	if err := pruneUnmanaged(ctx, r.Client, &rbacv1.ClusterRoleBindingList{}, "",
		map[string]struct{}{envNamespaceRBACName(&project): {}},
		func(l *rbacv1.ClusterRoleBindingList) []*rbacv1.ClusterRoleBinding {
			var items []*rbacv1.ClusterRoleBinding
			for i := range l.Items {
				slug, hasSlug := l.Items[i].Labels["enzarb.io/project"]
				org := l.Items[i].Labels["enzarb.io/org"]
				if !hasSlug || (slug == project.Spec.Slug && org == project.Spec.OrgID) {
					items = append(items, &l.Items[i])
				}
			}
			return items
		},
	); err != nil {
		return ctrl.Result{}, fmt.Errorf("prune stale cluster role bindings: %w", err)
	}

	// Prune the org-namespace "deployer" RoleBinding: workspaces must deploy
	// only via Environments (which get their own namespace-scoped binding to
	// enzarb-deployer), not directly in their own org namespace.
	if err := pruneUnmanaged(ctx, r.Client, &rbacv1.RoleBindingList{}, orgNS,
		map[string]struct{}{},
		func(l *rbacv1.RoleBindingList) []*rbacv1.RoleBinding {
			items := make([]*rbacv1.RoleBinding, len(l.Items))
			for i := range l.Items {
				items[i] = &l.Items[i]
			}
			return items
		},
	); err != nil {
		return ctrl.Result{}, fmt.Errorf("prune stale role bindings: %w", err)
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

	if err := r.ensureReferenceGrant(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure referencegrant: %w", err)
	}

	if err := r.ensureHTTPRoute(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure httproute: %w", err)
	}

	if err := r.ensureEnvContextConfigMap(ctx, orgNS, &project, envNamespaces); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure env context configmap: %w", err)
	}

	if err := r.ensureDeployEgressPolicy(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deploy egress network policy: %w", err)
	}

	if err := r.reconcileEnvironmentSuspension(ctx, orgNS, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("reconcile environment suspension: %w", err)
	}

	agentPath := agentPathFor(&project)
	if project.Spec.Suspended {
		project.Status.Phase = "Suspended"
		apimeta.SetStatusCondition(&project.Status.Conditions, metav1.Condition{
			Type:    "Ready",
			Status:  metav1.ConditionFalse,
			Reason:  "Suspended",
			Message: "project suspended; workspace and environments are scaled to zero",
		})
	} else {
		project.Status.Phase = "Running"
		apimeta.SetStatusCondition(&project.Status.Conditions, metav1.Condition{
			Type:   "Ready",
			Status: metav1.ConditionTrue,
			Reason: "Running",
		})
	}
	project.Status.ServiceAccountName = saName
	project.Status.AgentPath = agentPath
	if err := r.Status().Update(ctx, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("update status: %w", err)
	}

	logger.Info("project reconciled", "name", project.Name, "namespace", project.Namespace)
	// Workspace image drift (desiredWorkspaceImage vs runningWorkspaceImage) and
	// process-liveness state can only be detected here in ensureDeployment, which
	// only runs on reconcile. Nothing about a new workspace image release changes
	// a watched object on this Project, so without a periodic requeue a stable
	// project would only ever get re-checked when the operator pod itself
	// restarts — leaving version-update notifications stuck until then.
	return ctrl.Result{RequeueAfter: workspaceDriftRecheckInterval}, nil
}

// projectFinalizer guards cleanup of resources outside the project's namespace
// (enzarb-system HTTPRoute/Certificate) that owner-reference GC can't reach.
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

// suspendReplicasAnnotation records a tenant workload's replica count before
// it was scaled to zero for a project suspend, so resuming restores it
// exactly rather than assuming 1 (which would be wrong for anything
// intentionally run with multiple replicas).
const suspendReplicasAnnotation = "enzarb.io/pre-suspend-replicas"

// reconcileEnvironmentSuspension scales every child Environment's
// tenant-deployed workloads (Deployments/StatefulSets — resources the
// operator doesn't own; tenants create these themselves via kubectl/Helm) to
// zero when the project is suspended, and restores their prior replica counts
// when it's resumed. Neither the namespace nor any data is touched either way.
func (r *ProjectReconciler) reconcileEnvironmentSuspension(ctx context.Context, orgNS string, project *enzarbv1alpha1.Project) error {
	var envList enzarbv1alpha1.EnvironmentList
	if err := r.List(ctx, &envList, client.InNamespace(orgNS)); err != nil {
		return fmt.Errorf("list environments: %w", err)
	}
	for i := range envList.Items {
		env := &envList.Items[i]
		if env.Spec.ProjectRef.Name != project.Spec.Slug || env.Status.Namespace == "" {
			continue
		}
		if err := r.setWorkloadsSuspended(ctx, env.Status.Namespace, project.Spec.Suspended); err != nil {
			return fmt.Errorf("environment %s: %w", env.Name, err)
		}
	}
	return nil
}

func (r *ProjectReconciler) setWorkloadsSuspended(ctx context.Context, ns string, suspend bool) error {
	var deploys appsv1.DeploymentList
	if err := r.List(ctx, &deploys, client.InNamespace(ns)); err != nil {
		return fmt.Errorf("list deployments: %w", err)
	}
	for i := range deploys.Items {
		if err := r.setDeploymentSuspended(ctx, &deploys.Items[i], suspend); err != nil {
			return err
		}
	}
	var statefulSets appsv1.StatefulSetList
	if err := r.List(ctx, &statefulSets, client.InNamespace(ns)); err != nil {
		return fmt.Errorf("list statefulsets: %w", err)
	}
	for i := range statefulSets.Items {
		if err := r.setStatefulSetSuspended(ctx, &statefulSets.Items[i], suspend); err != nil {
			return err
		}
	}
	return nil
}

func (r *ProjectReconciler) setDeploymentSuspended(ctx context.Context, deploy *appsv1.Deployment, suspend bool) error {
	if suspend {
		if deploy.Spec.Replicas != nil && *deploy.Spec.Replicas == 0 {
			return nil
		}
		current := int32(1)
		if deploy.Spec.Replicas != nil {
			current = *deploy.Spec.Replicas
		}
		if deploy.Annotations == nil {
			deploy.Annotations = map[string]string{}
		}
		deploy.Annotations[suspendReplicasAnnotation] = strconv.Itoa(int(current))
		zero := int32(0)
		deploy.Spec.Replicas = &zero
		return r.Update(ctx, deploy)
	}
	saved, ok := deploy.Annotations[suspendReplicasAnnotation]
	if !ok {
		return nil
	}
	n, err := strconv.ParseInt(saved, 10, 32)
	if err != nil {
		n = 1
	}
	restored := int32(n)
	deploy.Spec.Replicas = &restored
	delete(deploy.Annotations, suspendReplicasAnnotation)
	return r.Update(ctx, deploy)
}

func (r *ProjectReconciler) setStatefulSetSuspended(ctx context.Context, sts *appsv1.StatefulSet, suspend bool) error {
	if suspend {
		if sts.Spec.Replicas != nil && *sts.Spec.Replicas == 0 {
			return nil
		}
		current := int32(1)
		if sts.Spec.Replicas != nil {
			current = *sts.Spec.Replicas
		}
		if sts.Annotations == nil {
			sts.Annotations = map[string]string{}
		}
		sts.Annotations[suspendReplicasAnnotation] = strconv.Itoa(int(current))
		zero := int32(0)
		sts.Spec.Replicas = &zero
		return r.Update(ctx, sts)
	}
	saved, ok := sts.Annotations[suspendReplicasAnnotation]
	if !ok {
		return nil
	}
	n, err := strconv.ParseInt(saved, 10, 32)
	if err != nil {
		n = 1
	}
	restored := int32(n)
	sts.Spec.Replicas = &restored
	delete(sts.Annotations, suspendReplicasAnnotation)
	return r.Update(ctx, sts)
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

	rbName := fmt.Sprintf("enzarb-%s-%s-deployer", project.Spec.OrgID, project.Spec.Slug)
	if err := r.deleteIfExists(ctx, &rbacv1.RoleBinding{
		ObjectMeta: metav1.ObjectMeta{Name: rbName, Namespace: ns},
	}); err != nil {
		return ctrl.Result{}, fmt.Errorf("delete role binding: %w", err)
	}
	if err := r.deleteCapsuleTenant(ctx, project); err != nil {
		return ctrl.Result{}, fmt.Errorf("delete capsule tenant: %w", err)
	}
	envNSName := envNamespaceRBACName(project)
	if err := r.deleteIfExists(ctx, &rbacv1.ClusterRoleBinding{
		ObjectMeta: metav1.ObjectMeta{Name: envNSName},
	}); err != nil {
		return ctrl.Result{}, fmt.Errorf("delete env-ns cluster role binding: %w", err)
	}
	if err := r.deleteIfExists(ctx, &rbacv1.ClusterRole{
		ObjectMeta: metav1.ObjectMeta{Name: envNSName},
	}); err != nil {
		return ctrl.Result{}, fmt.Errorf("delete env-ns cluster role: %w", err)
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

// deployEgressPolicyName is the per-project NetworkPolicy in the org namespace
// that lets a project's workspace pods reach that project's deploy namespaces.
func deployEgressPolicyName(slug string) string {
	return fmt.Sprintf("enzarb-deploy-egress-%s", slug)
}

// ensureDeployEgressPolicy adds an egress rule (additive to the org-wide
// enzarb-workspace-isolation default-deny) allowing only this project's
// workspace pods to reach this project's own deploy (environment) namespaces.
// Sibling projects' workspaces and other orgs' deploy namespaces stay blocked;
// the deploy-side enzarb-deploy-isolation ingress enforces the same pairing
// from the other direction.
func (r *ProjectReconciler) ensureDeployEgressPolicy(ctx context.Context, ns string, project *enzarbv1alpha1.Project) error {
	if os.Getenv("NETWORK_POLICY_ENABLED") == "false" {
		return nil
	}

	desired := &networkingv1.NetworkPolicy{
		ObjectMeta: metav1.ObjectMeta{
			Name:      deployEgressPolicyName(project.Spec.Slug),
			Namespace: ns,
			Labels:    projectLabels(project),
		},
		Spec: networkingv1.NetworkPolicySpec{
			PodSelector: metav1.LabelSelector{
				MatchLabels: map[string]string{"enzarb.io/project": project.Spec.Slug},
			},
			PolicyTypes: []networkingv1.PolicyType{networkingv1.PolicyTypeEgress},
			Egress: []networkingv1.NetworkPolicyEgressRule{
				{To: []networkingv1.NetworkPolicyPeer{{
					NamespaceSelector: &metav1.LabelSelector{
						MatchLabels: map[string]string{
							"enzarb.io/type":         "deploy",
							"enzarb.io/org-id":       project.Spec.OrgID,
							"enzarb.io/project-slug": project.Spec.Slug,
						},
					},
				}}},
			},
		},
	}

	existing := &networkingv1.NetworkPolicy{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: desired.Name}, existing)
	if errors.IsNotFound(err) {
		if err := controllerutil.SetControllerReference(project, desired, r.Scheme); err != nil {
			return err
		}
		return r.Create(ctx, desired)
	}
	if err != nil {
		return err
	}
	existing.Spec = desired.Spec
	existing.Labels = desired.Labels
	return r.Update(ctx, existing)
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

// envNamespaceRBACName names the per-project ClusterRole and ClusterRoleBinding
// that let the workspace SA `kubectl get` its environments' deploy namespaces.
func envNamespaceRBACName(project *enzarbv1alpha1.Project) string {
	return fmt.Sprintf("enzarb-%s-%s-env-ns", project.Spec.OrgID, project.Spec.Slug)
}

// projectEnvNamespaces returns the sorted deploy namespaces of every provisioned
// Environment belonging to the project.
func (r *ProjectReconciler) projectEnvNamespaces(ctx context.Context, orgNS string, project *enzarbv1alpha1.Project) ([]string, error) {
	var envList enzarbv1alpha1.EnvironmentList
	if err := r.List(ctx, &envList, client.InNamespace(orgNS)); err != nil {
		return nil, fmt.Errorf("list environments: %w", err)
	}
	var namespaces []string
	for i := range envList.Items {
		env := &envList.Items[i]
		if env.Spec.ProjectRef.Name == project.Spec.Slug && env.Status.Namespace != "" {
			namespaces = append(namespaces, env.Status.Namespace)
		}
	}
	sort.Strings(namespaces)
	return namespaces, nil
}

// ensureEnvNamespaceRBAC maintains a ClusterRole granting `get` on the
// project's environment deploy namespaces (by name — namespaces are
// cluster-scoped, so a namespaced Role can't express this) and a
// ClusterRoleBinding attaching it to the workspace ServiceAccount. RBAC can't
// name-scope `list`, so `kubectl get ns` without arguments stays forbidden;
// `kubectl get ns <deploy-ns>` works. Both objects are deleted when the
// project has no provisioned environments (and on project deletion).
func (r *ProjectReconciler) ensureEnvNamespaceRBAC(ctx context.Context, orgNS, saName string, project *enzarbv1alpha1.Project, envNamespaces []string) error {
	name := envNamespaceRBACName(project)

	if len(envNamespaces) == 0 {
		if err := r.deleteIfExists(ctx, &rbacv1.ClusterRoleBinding{ObjectMeta: metav1.ObjectMeta{Name: name}}); err != nil {
			return err
		}
		return r.deleteIfExists(ctx, &rbacv1.ClusterRole{ObjectMeta: metav1.ObjectMeta{Name: name}})
	}

	desiredRole := &rbacv1.ClusterRole{
		ObjectMeta: metav1.ObjectMeta{Name: name, Labels: projectLabels(project)},
		Rules: []rbacv1.PolicyRule{{
			APIGroups:     []string{""},
			Resources:     []string{"namespaces"},
			Verbs:         []string{"get"},
			ResourceNames: envNamespaces,
		}},
	}
	existingRole := &rbacv1.ClusterRole{}
	err := r.Get(ctx, types.NamespacedName{Name: name}, existingRole)
	switch {
	case errors.IsNotFound(err):
		if err := r.Create(ctx, desiredRole); err != nil {
			return fmt.Errorf("create env-ns clusterrole: %w", err)
		}
	case err != nil:
		return err
	case !reflect.DeepEqual(existingRole.Rules, desiredRole.Rules):
		existingRole.Rules = desiredRole.Rules
		existingRole.Labels = desiredRole.Labels
		if err := r.Update(ctx, existingRole); err != nil {
			return fmt.Errorf("update env-ns clusterrole: %w", err)
		}
	}

	binding := &rbacv1.ClusterRoleBinding{}
	err = r.Get(ctx, types.NamespacedName{Name: name}, binding)
	if errors.IsNotFound(err) {
		binding = &rbacv1.ClusterRoleBinding{
			ObjectMeta: metav1.ObjectMeta{Name: name, Labels: projectLabels(project)},
			RoleRef: rbacv1.RoleRef{
				APIGroup: rbacv1.GroupName,
				Kind:     "ClusterRole",
				Name:     name,
			},
			Subjects: []rbacv1.Subject{{
				Kind:      "ServiceAccount",
				Name:      saName,
				Namespace: orgNS,
			}},
		}
		return r.Create(ctx, binding)
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

const forceRestartAnnotation = "enzarb.io/force-workspace-restart"

// How often a steady-state project gets re-reconciled purely to notice
// workspace image drift and re-check process liveness (see the comment on
// the happy-path return in Reconcile).
const workspaceDriftRecheckInterval = 5 * time.Minute

func (r *ProjectReconciler) ensureDeployment(ctx context.Context, ns, saName, pvcName, orgSlug string, project *enzarbv1alpha1.Project) error {
	log := log.FromContext(ctx)
	deployName := fmt.Sprintf("project-%s", project.Spec.Slug)
	deploy := &appsv1.Deployment{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: deployName}, deploy)
	desired := r.buildDeployment(ns, deployName, saName, pvcName, orgSlug, project)
	if err := controllerutil.SetControllerReference(project, desired, r.Scheme); err != nil {
		return err
	}

	desiredImage := workspaceImage()
	project.Status.DesiredWorkspaceImage = desiredImage

	if errors.IsNotFound(err) {
		project.Status.RunningWorkspaceImage = desiredImage
		apimeta.RemoveStatusCondition(&project.Status.Conditions, "WorkspaceUpdatePending")
		return r.Create(ctx, desired)
	}
	if err != nil {
		return err
	}

	// Determine the image currently running in the existing deployment.
	runningImage := desiredImage
	for _, c := range deploy.Spec.Template.Spec.Containers {
		if c.Name == "workspace" {
			runningImage = c.Image
			break
		}
	}
	project.Status.RunningWorkspaceImage = runningImage

	// If image hasn't changed, apply any other spec drift and clear the condition.
	if runningImage == desiredImage {
		apimeta.RemoveStatusCondition(&project.Status.Conditions, "WorkspaceUpdatePending")
		deploy.Spec = desired.Spec
		return r.Update(ctx, deploy)
	}

	// Suspended projects have 0 replicas so no pods are running — the image
	// update is always safe and should be applied immediately so the workspace
	// is already up-to-date when the project is resumed.
	if project.Spec.Suspended {
		apimeta.RemoveStatusCondition(&project.Status.Conditions, "WorkspaceUpdatePending")
		project.Status.RunningWorkspaceImage = desiredImage
		deploy.Spec = desired.Spec
		return r.Update(ctx, deploy)
	}

	// Image update needed. Honour an explicit user-initiated restart request.
	forceRequested := project.Annotations[forceRestartAnnotation] == "true"
	if forceRequested {
		// Remove the annotation so the override is one-shot.
		patch := []byte(`{"metadata":{"annotations":{"` + forceRestartAnnotation + `":null}}}`)
		if pErr := r.Patch(ctx, project, client.RawPatch(types.MergePatchType, patch)); pErr != nil {
			log.Error(pErr, "failed to remove force-restart annotation; proceeding anyway")
		}
	}

	if !forceRequested {
		hasProcesses, checkErr := r.checkRunningProcesses(ctx, ns, project.Spec.Slug)
		reason := "RunningProcesses"
		if checkErr != nil {
			// Unknown process state (agent unreachable, still starting up, transient
			// network blip, etc.) must NOT be treated as "safe to restart" — a
			// spurious pending-update banner is far cheaper than silently killing a
			// user's terminal session or dev server because we guessed wrong.
			log.Info("could not reach agent to check processes; assuming processes may be running", "error", checkErr)
			hasProcesses = true
			reason = "ProcessCheckFailed"
		}
		if hasProcesses {
			changelog := workspaceChangelogRange(runningImage, desiredImage)
			apimeta.SetStatusCondition(&project.Status.Conditions, metav1.Condition{
				Type:               "WorkspaceUpdatePending",
				Status:             metav1.ConditionTrue,
				Reason:             reason,
				Message:            changelog,
				ObservedGeneration: project.Generation,
			})
			return nil
		}
	}

	apimeta.RemoveStatusCondition(&project.Status.Conditions, "WorkspaceUpdatePending")
	project.Status.RunningWorkspaceImage = desiredImage
	deploy.Spec = desired.Spec
	return r.Update(ctx, deploy)
}

// checkRunningProcesses queries the workspace agent's internal /processes
// endpoint. Returns true when at least one process is in Running state or at
// least one ACP agent session is active (Live). On network failure (agent not
// yet up, pod restarting) returns (false, err); callers should treat an error
// as "no running processes" for safety.
func (r *ProjectReconciler) checkRunningProcesses(ctx context.Context, ns, slug string) (bool, error) {
	url := fmt.Sprintf("http://project-%s.%s.svc.cluster.local:9090/processes", slug, ns)
	reqCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(reqCtx, http.MethodGet, url, nil)
	if err != nil {
		return false, err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return false, err
	}
	defer func() { _ = resp.Body.Close() }()
	var result struct {
		Running  int `json:"running"`
		Sessions int `json:"sessions"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return false, err
	}
	return result.Running > 0 || result.Sessions > 0, nil
}

func (r *ProjectReconciler) buildDeployment(ns, name, saName, pvcName, orgSlug string, project *enzarbv1alpha1.Project) *appsv1.Deployment {
	labels := projectLabels(project)
	replicas := int32(1)
	if project.Spec.Suspended {
		replicas = 0
	}

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

	nodeSelector := workspaceNodeSelector()
	var tolerations []corev1.Toleration
	gpuResources := corev1.ResourceList{}
	if project.Spec.GPUEnabled {
		if nodeSelector == nil {
			nodeSelector = map[string]string{}
		}
		nodeSelector["nvidia.com/gpu.present"] = "true"
		tolerations = []corev1.Toleration{{
			Key:      "nvidia.com/gpu",
			Operator: corev1.TolerationOpExists,
			Effect:   corev1.TaintEffectNoSchedule,
		}}
		gpuResources[corev1.ResourceName("nvidia.com/gpu")] = resource.MustParse("1")
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
					NodeSelector:       nodeSelector,
					Tolerations:        tolerations,
					// Make the in-workspace hostname the project slug (a valid
					// DNS-1123 label) instead of the generated pod name, so shell
					// prompts read e.g. `user@krustbe` rather than the replica hash.
					Hostname: project.Spec.Slug,
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
								// Base origin for the agent: drives CORS, JWKS/revocation
								// fetch, and the expected JWT issuer. Must match the app's
								// configured domain (Helm-driven) so issued tokens validate.
								{Name: "APP_ORIGIN", Value: fmt.Sprintf("https://%s", r.Domain)},
								// Preconfigured registry + git coordinates (GHCR-style). The
								// workspace's credential helpers auth to these automatically; the
								// project may only push/pull within its own <orgSlug>/<slug> prefix.
								{Name: "ENZARB_REGISTRY", Value: fmt.Sprintf("registry.%s", r.Domain)},
								{Name: "REGISTRY", Value: fmt.Sprintf("registry.%s/%s/%s", r.Domain, orgSlug, project.Spec.Slug)},
								{Name: "ENZARB_IMAGE", Value: fmt.Sprintf("registry.%s/%s/%s", r.Domain, orgSlug, project.Spec.Slug)},
								// buildkitd sidecar speaks the BuildKit gRPC API, not the
								// Docker daemon API — clients reach it via BUILDKIT_HOST.
								{Name: "BUILDKIT_HOST", Value: "tcp://localhost:1234"},
								{Name: "HOME", Value: "/home/user"},
								// XDG dirs pinned under HOME so tools write to the PVC-backed
								// home dir instead of system paths on the read-only root filesystem.
								{Name: "XDG_DATA_HOME", Value: "/home/user/.local/share"},
								{Name: "XDG_CACHE_HOME", Value: "/home/user/.cache"},
								{Name: "XDG_CONFIG_HOME", Value: "/home/user/.config"},
								{Name: "XDG_STATE_HOME", Value: "/home/user/.local/state"},
								// kubectl/helm route through capsule-proxy (tenant-filtered
								// cluster-scoped lists); the kubeconfig ConfigMap falls back
								// to the direct API server when the proxy isn't deployed.
								{Name: "KUBECONFIG", Value: workspaceKubeconfigMountPath + "/config"},
							},
							EnvFrom: []corev1.EnvFromSource{
								{
									SecretRef: &corev1.SecretEnvSource{
										LocalObjectReference: corev1.LocalObjectReference{
											Name: fmt.Sprintf("%s-user-env-secrets", project.Spec.OrgID),
										},
										Optional: boolPtr(true),
									},
								},
								{
									SecretRef: &corev1.SecretEnvSource{
										LocalObjectReference: corev1.LocalObjectReference{
											Name: fmt.Sprintf("%s-project-env-secrets", project.Spec.Slug),
										},
										Optional: boolPtr(true),
									},
								},
							},
							Ports: []corev1.ContainerPort{
								{Name: "agent-external", ContainerPort: 8080},
								{Name: "agent-internal", ContainerPort: 9090},
							},
							Resources: corev1.ResourceRequirements{
								Requests: func() corev1.ResourceList {
									r := corev1.ResourceList{
										corev1.ResourceCPU:    cpuReq,
										corev1.ResourceMemory: memReq,
									}
									for k, v := range gpuResources {
										r[k] = v
									}
									return r
								}(),
								Limits: func() corev1.ResourceList {
									l := corev1.ResourceList{
										corev1.ResourceCPU:    cpuLim,
										corev1.ResourceMemory: memLim,
									}
									for k, v := range gpuResources {
										l[k] = v
									}
									return l
								}(),
							},
							VolumeMounts: []corev1.VolumeMount{
								{Name: "home", MountPath: "/home/user"},
								{Name: "tmp", MountPath: "/tmp"},
								{Name: "registry-token", MountPath: "/var/run/secrets/enzarb/registry"},
								{Name: "env-context", MountPath: "/var/run/enzarb/env", ReadOnly: true},
								{Name: "kubeconfig", MountPath: workspaceKubeconfigMountPath, ReadOnly: true},
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
							Name:         "buildkitd",
							Image:        "moby/buildkit:rootless",
							Args:         buildkitArgs(),
							Ports:        []corev1.ContainerPort{{Name: "buildkitd", ContainerPort: 1234}},
							VolumeMounts: buildkitVolumeMounts(),
							SecurityContext: &corev1.SecurityContext{
								RunAsUser:  int64Ptr(1000),
								RunAsGroup: int64Ptr(1000),
								// Rootless buildkit sets up a user-namespace worker, which
								// needs syscalls (clone(CLONE_NEWUSER), clone3, unshare,
								// mount, setns, ...) that RuntimeDefault gates behind
								// CAP_SYS_ADMIN. A custom per-node profile (installed by the
								// enzarb-node-profiles-installer DaemonSet, see
								// deploy/system/node-profiles.yaml) permits those while still
								// denying the escape-adjacent syscalls (bpf, perf_event_open,
								// open_by_handle_at, module loading, ...) that Unconfined
								// would allow.
								//
								// AppArmor stays Unconfined: the container-default AppArmor
								// profile (enforced on Ubuntu nodes) denies the mount
								// operations RootlessKit performs, so a tailored per-node
								// profile is required before this can be tightened.
								SeccompProfile: &corev1.SeccompProfile{
									Type:             corev1.SeccompProfileTypeLocalhost,
									LocalhostProfile: strPtr("enzarb/buildkitd.json"),
								},
								AppArmorProfile: &corev1.AppArmorProfile{
									Type: corev1.AppArmorProfileTypeUnconfined,
								},
							},
						},
					},
					Volumes: append([]corev1.Volume{
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
							Name: "env-context",
							VolumeSource: corev1.VolumeSource{
								ConfigMap: &corev1.ConfigMapVolumeSource{
									LocalObjectReference: corev1.LocalObjectReference{
										Name: fmt.Sprintf("%s-env-context", project.Spec.Slug),
									},
									// optional=true so pod starts even before the ConfigMap is created
									// (rare timing window on first reconcile).
									Optional: boolPtr(true),
								},
							},
						},
						{
							Name: "kubeconfig",
							VolumeSource: corev1.VolumeSource{
								ConfigMap: &corev1.ConfigMapVolumeSource{
									LocalObjectReference: corev1.LocalObjectReference{
										Name: workspaceKubeconfigName(project.Spec.Slug),
									},
									Optional: boolPtr(true),
								},
							},
						},
					}, buildkitConfigVolumeSlice()...),
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

// ensureEnvContextConfigMap creates or updates a ConfigMap named
// "<slug>-env-context" in the org namespace. Its context.sh key exports
// KUBE_NAMESPACE for the project's default environment (set via the
// enzarb.io/default-environment annotation). Mounted into the workspace pod, it
// is updated in-place by Kubernetes (~60 s propagation) without a pod restart.
func (r *ProjectReconciler) ensureEnvContextConfigMap(ctx context.Context, ns string, project *enzarbv1alpha1.Project, envNamespaces []string) error {
	cmName := fmt.Sprintf("%s-env-context", project.Spec.Slug)

	// Determine which environment is the default (annotation value = env slug).
	envNamespace := ""
	if defaultEnvSlug, ok := project.Annotations["enzarb.io/default-environment"]; ok && defaultEnvSlug != "" {
		envName := fmt.Sprintf("%s-%s", project.Spec.Slug, defaultEnvSlug)
		var env enzarbv1alpha1.Environment
		if err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: envName}, &env); err == nil {
			envNamespace = env.Status.Namespace
		}
	}

	var contextSh string
	if envNamespace != "" {
		contextSh = fmt.Sprintf("export POD_NAMESPACE=%s\n", envNamespace)
	} else {
		contextSh = "# no default environment set\n"
	}
	// All of the project's environment deploy namespaces, so workspace tooling
	// can enumerate them (RBAC only grants `get` on these by name; an
	// unfiltered `kubectl get ns` list is not permitted).
	if len(envNamespaces) > 0 {
		contextSh += fmt.Sprintf("export ENZARB_ENV_NAMESPACES=%q\n", strings.Join(envNamespaces, " "))
	}

	cm := &corev1.ConfigMap{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: cmName}, cm)
	if errors.IsNotFound(err) {
		cm = &corev1.ConfigMap{
			ObjectMeta: metav1.ObjectMeta{
				Name:      cmName,
				Namespace: ns,
				Labels:    projectLabels(project),
			},
			Data: map[string]string{"context.sh": contextSh},
		}
		if err := controllerutil.SetControllerReference(project, cm, r.Scheme); err != nil {
			return err
		}
		return r.Create(ctx, cm)
	}
	if err != nil {
		return err
	}
	if cm.Data["context.sh"] == contextSh {
		return nil
	}
	cm.Data = map[string]string{"context.sh": contextSh}
	return r.Update(ctx, cm)
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
	requestTimeout := gatewayv1.Duration("60s")

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
			// Envoy Gateway's default request timeout (15s) is shorter than the
			// agent's own ACP_REQUEST_TIMEOUT (30s, see agent/src/acp/store.rs).
			// A slow-but-healthy session/list call (e.g. while the ACP relay is
			// busy resuming other sessions) would otherwise be killed by the
			// gateway as a 504 before the agent gets a chance to respond.
			Timeouts: &gatewayv1.HTTPRouteTimeouts{
				Request: &requestTimeout,
			},
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

// pruneLegacyProjectCertificate removes the obsolete per-project apex-domain
// Certificate (and its output Secret) from enzarb-system. Deleting the
// Certificate cascades to its owned CertificateRequest and temporary key Secret;
// buildkitArgs returns the buildkitd sidecar arguments, adding the config file
// with registry mirror entries when the pull-through mirror is enabled. The
// ConfigMap is created per org namespace by the Organization reconciler.
func buildkitArgs() []string {
	args := []string{
		"--addr", "tcp://0.0.0.0:1234",
		// Required to run rootless buildkitd in an unprivileged container.
		"--oci-worker-no-process-sandbox",
	}
	if _, enabled := mirrorEnabled(); enabled {
		args = append(args, "--config", "/etc/buildkit/buildkitd.toml")
	}
	return args
}

func buildkitVolumeMounts() []corev1.VolumeMount {
	if _, enabled := mirrorEnabled(); !enabled {
		return nil
	}
	return []corev1.VolumeMount{{Name: "buildkit-config", MountPath: "/etc/buildkit", ReadOnly: true}}
}

// buildkitConfigVolumeSlice returns the ConfigMap volume for buildkitd.toml,
// or an empty slice when the mirror is disabled.
func buildkitConfigVolumeSlice() []corev1.Volume {
	if _, enabled := mirrorEnabled(); !enabled {
		return nil
	}
	return []corev1.Volume{{
		Name: "buildkit-config",
		VolumeSource: corev1.VolumeSource{
			ConfigMap: &corev1.ConfigMapVolumeSource{
				LocalObjectReference: corev1.LocalObjectReference{Name: buildkitConfigMapName},
				// optional=true so the pod starts even before the Organization
				// reconciler has created the ConfigMap (buildkitd treats a
				// missing --config file as empty config and pulls direct).
				Optional: boolPtr(true),
			},
		},
	}}
}

func projectLabels(p *enzarbv1alpha1.Project) map[string]string {
	return map[string]string{
		"app.kubernetes.io/managed-by": "enzarb-operator",
		"enzarb.io/project":            p.Spec.Slug,
		"enzarb.io/org":                p.Spec.OrgID,
	}
}

// workspaceChangelogRange returns the combined workspace changelog covering all
// versions strictly between runningImage and desiredImage. It reads
// WORKSPACE_CHANGELOGS (a JSON object of { "vX.Y.Z": "changelog text" }) and
// concatenates entries whose versions fall in the half-open range
// (runningTag, desiredTag]. Falls back to the legacy WORKSPACE_CHANGELOG env
// var if WORKSPACE_CHANGELOGS is absent or unparseable.
func workspaceChangelogRange(runningImage, desiredImage string) string {
	raw := os.Getenv("WORKSPACE_CHANGELOGS")
	if raw == "" {
		return os.Getenv("WORKSPACE_CHANGELOG")
	}
	var changelogs map[string]string
	if err := json.Unmarshal([]byte(raw), &changelogs); err != nil {
		return os.Getenv("WORKSPACE_CHANGELOG")
	}

	runningTag := imageTag(runningImage)
	desiredTag := imageTag(desiredImage)
	if runningTag == "" || desiredTag == "" || runningTag == desiredTag {
		return ""
	}

	// Collect and sort versions that fall in (runningTag, desiredTag].
	// Semver comparison: split "vMAJOR.MINOR.PATCH" and compare numerically.
	var relevant []string
	for ver := range changelogs {
		if semverGT(ver, runningTag) && (semverEQ(ver, desiredTag) || semverLT(ver, desiredTag)) {
			relevant = append(relevant, ver)
		}
	}
	// Sort ascending so oldest changes come first.
	for i := 0; i < len(relevant); i++ {
		for j := i + 1; j < len(relevant); j++ {
			if semverGT(relevant[i], relevant[j]) {
				relevant[i], relevant[j] = relevant[j], relevant[i]
			}
		}
	}

	var parts []string
	for _, ver := range relevant {
		if cl := changelogs[ver]; cl != "" {
			parts = append(parts, cl)
		}
	}
	if len(parts) == 0 {
		return ""
	}
	result := ""
	for i, p := range parts {
		if i > 0 {
			result += "\n"
		}
		result += p
	}
	return result
}

// imageTag extracts the tag portion from an image reference (the part after the last colon).
func imageTag(image string) string {
	for i := len(image) - 1; i >= 0; i-- {
		if image[i] == ':' {
			return image[i+1:]
		}
	}
	return ""
}

// semverParts parses a "vMAJOR.MINOR.PATCH" tag into its numeric components.
func semverParts(tag string) (int, int, int) {
	s := tag
	if len(s) > 0 && s[0] == 'v' {
		s = s[1:]
	}
	var major, minor, patch int
	_, _ = fmt.Sscanf(s, "%d.%d.%d", &major, &minor, &patch)
	return major, minor, patch
}

func semverGT(a, b string) bool {
	ma, na, pa := semverParts(a)
	mb, nb, pb := semverParts(b)
	if ma != mb {
		return ma > mb
	}
	if na != nb {
		return na > nb
	}
	return pa > pb
}

func semverLT(a, b string) bool { return semverGT(b, a) }
func semverEQ(a, b string) bool { return !semverGT(a, b) && !semverLT(a, b) }

// workspaceImage returns the workspace container image, pinned via the
// WORKSPACE_IMAGE env (set from the Helm chart's workspace.image values).
// Falls back to a floating tag for local/dev when unset.
func workspaceImage() string {
	if img := os.Getenv("WORKSPACE_IMAGE"); img != "" {
		return img
	}
	return "ghcr.io/enzarb/workspace:latest"
}

// workspaceNodeSelector returns the base nodeSelector applied to every
// workspace pod, sourced from WORKSPACE_NODE_SELECTOR (set from the Helm
// chart's workspace.nodeSelector value) as comma-separated key=value pairs,
// e.g. "enzarb.io/avx2=true". Empty/unset means no constraint. This exists
// because the workspace image bundles a Bun-compiled binary that segfaults
// on CPUs without AVX2 — clusters with older nodes label the capable ones
// and set this so workspace pods only land there.
func workspaceNodeSelector() map[string]string {
	raw := os.Getenv("WORKSPACE_NODE_SELECTOR")
	if raw == "" {
		return nil
	}
	selector := map[string]string{}
	for _, pair := range strings.Split(raw, ",") {
		pair = strings.TrimSpace(pair)
		if pair == "" {
			continue
		}
		k, v, ok := strings.Cut(pair, "=")
		if !ok {
			continue
		}
		selector[strings.TrimSpace(k)] = strings.TrimSpace(v)
	}
	if len(selector) == 0 {
		return nil
	}
	return selector
}

func int64Ptr(i int64) *int64 { return &i }

func strPtr(s string) *string { return &s }
func boolPtr(b bool) *bool    { return &b }
