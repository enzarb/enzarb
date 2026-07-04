package controller

import (
	"context"
	"fmt"
	"os"
	"time"

	corev1 "k8s.io/api/core/v1"
	networkingv1 "k8s.io/api/networking/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	apimeta "k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/intstr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/log"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
)

// organizationFinalizer blocks deletion of an Organization (and therefore its
// namespace) while Projects still live in that namespace, forcing explicit
// per-project deletion first.
const organizationFinalizer = "enzarb.io/organization-protection"

// defaultRequeue is how long to wait before re-checking a blocked org teardown.
const defaultRequeue = 30 * time.Second

type OrganizationReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

func (r *OrganizationReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&enzarbv1alpha1.Organization{}).
		Complete(r)
}

func orgNamespaceName(orgID string) string {
	return fmt.Sprintf("user-%s", orgID)
}

func (r *OrganizationReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var org enzarbv1alpha1.Organization
	if err := r.Get(ctx, req.NamespacedName, &org); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	nsName := orgNamespaceName(org.Spec.OrgID)

	if !org.DeletionTimestamp.IsZero() {
		return r.reconcileDelete(ctx, &org, nsName)
	}

	// Ensure the finalizer is present before we create anything we need to
	// guard on teardown.
	if !controllerutil.ContainsFinalizer(&org, organizationFinalizer) {
		controllerutil.AddFinalizer(&org, organizationFinalizer)
		if err := r.Update(ctx, &org); err != nil {
			return ctrl.Result{}, fmt.Errorf("add finalizer: %w", err)
		}
		return ctrl.Result{Requeue: true}, nil
	}

	// Soft-deleted (retention window): hold the org and its projects until the
	// purge time, then cascade a hard delete.
	if purgeTime, ok := purgeAfter(&org); ok {
		return r.reconcileOrgRetention(ctx, &org, nsName, purgeTime)
	}

	if err := r.ensureNamespace(ctx, &org, nsName); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure namespace: %w", err)
	}

	// Isolate the workspace namespace: deny all ingress except the system
	// namespace (operator process checks on :9090) and the gateway data-plane
	// (user-facing agent API on :8080). This stops the unauthenticated agent
	// internal port from being reachable by arbitrary cluster pods.
	if err := r.ensureWorkspaceNetworkPolicy(ctx, nsName); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure workspace network policy: %w", err)
	}

	// Prune any other operator-managed NetworkPolicies from prior versions,
	// retaining the workspace isolation policy created above and each project's
	// deploy-egress policy (owned by the Project reconciler).
	expectedPolicies := map[string]struct{}{workspaceNetworkPolicyName: {}}
	var projects enzarbv1alpha1.ProjectList
	if err := r.List(ctx, &projects, client.InNamespace(nsName)); err != nil {
		return ctrl.Result{}, fmt.Errorf("list projects: %w", err)
	}
	for i := range projects.Items {
		expectedPolicies[deployEgressPolicyName(projects.Items[i].Spec.Slug)] = struct{}{}
	}
	if err := pruneUnmanaged(ctx, r.Client,
		&networkingv1.NetworkPolicyList{},
		nsName,
		expectedPolicies,
		func(l *networkingv1.NetworkPolicyList) []*networkingv1.NetworkPolicy {
			out := make([]*networkingv1.NetworkPolicy, len(l.Items))
			for i := range l.Items {
				out[i] = &l.Items[i]
			}
			return out
		},
	); err != nil {
		return ctrl.Result{}, fmt.Errorf("prune network policies: %w", err)
	}

	org.Status.Namespace = nsName
	org.Status.Phase = "Ready"
	apimeta.SetStatusCondition(&org.Status.Conditions, metav1.Condition{
		Type:    "Ready",
		Status:  metav1.ConditionTrue,
		Reason:  "NamespaceReady",
		Message: fmt.Sprintf("namespace %s is provisioned", nsName),
	})
	if err := r.Status().Update(ctx, &org); err != nil {
		return ctrl.Result{}, fmt.Errorf("update status: %w", err)
	}

	logger.Info("organization reconciled", "org", org.Spec.Slug, "namespace", nsName)
	return ctrl.Result{}, nil
}

// ensureNamespace creates the org namespace, or adopts an existing one by
// (re)applying the ownership labels so a namespace originally created by the
// old Project reconciler becomes owned by this Organization.
func (r *OrganizationReconciler) ensureNamespace(ctx context.Context, org *enzarbv1alpha1.Organization, name string) error {
	labels := orgNamespaceLabels(org)
	ns := &corev1.Namespace{}
	err := r.Get(ctx, types.NamespacedName{Name: name}, ns)
	if apierrors.IsNotFound(err) {
		return r.Create(ctx, &corev1.Namespace{
			ObjectMeta: metav1.ObjectMeta{Name: name, Labels: labels},
		})
	}
	if err != nil {
		return err
	}
	// Adopt: reconcile labels if drifted.
	changed := false
	if ns.Labels == nil {
		ns.Labels = map[string]string{}
	}
	for k, v := range labels {
		if ns.Labels[k] != v {
			ns.Labels[k] = v
			changed = true
		}
	}
	if changed {
		return r.Update(ctx, ns)
	}
	return nil
}

// workspaceNetworkPolicyName is the operator-managed ingress policy that
// isolates workspace (org) namespaces.
const workspaceNetworkPolicyName = "enzarb-workspace-isolation"

// ensureWorkspaceNetworkPolicy applies an ingress + egress NetworkPolicy to the
// workspace (org) namespace. The agent exposes an unauthenticated internal port
// (:9090) and a JWT-authenticated external port (:8080); without a policy both
// are reachable by any pod in the cluster.
//
// Ingress default-denies and admits only:
//   - the system namespace (enzarb-system) — the operator polls :9090 for
//     running-process checks, so it may reach both agent ports;
//   - the gateway data-plane namespace — the Envoy proxy routes user traffic to
//     the agent's external :8080.
//
// Sibling projects in the same org namespace are intentionally not admitted, so
// one project cannot reach another's agent.
//
// Egress default-denies and admits only DNS, the gateway data-plane (the
// authenticated path to the in-cluster registry/git/app services, all reached
// by hostname through the gateway), and the public internet with the cluster's
// own pod/service CIDRs carved out. That carve-out is what prevents a workspace
// from opening a direct connection to control-plane pods it should never touch
// — Postgres, authd, the operator, the app — rather than relying on those
// services' own credentials as the only line of defense. Broad outbound access
// (git, mise, package registries on the public internet) still works.
func (r *OrganizationReconciler) ensureWorkspaceNetworkPolicy(ctx context.Context, nsName string) error {
	if os.Getenv("NETWORK_POLICY_ENABLED") == "false" {
		return nil
	}

	systemNS := os.Getenv("SYSTEM_NAMESPACE")
	if systemNS == "" {
		systemNS = "enzarb-system"
	}
	gatewayNS := os.Getenv("GATEWAY_NAMESPACE")
	if gatewayNS == "" {
		gatewayNS = "envoy-gateway-system"
	}
	podCIDR := os.Getenv("CLUSTER_POD_CIDR")
	if podCIDR == "" {
		podCIDR = "10.42.0.0/16"
	}
	svcCIDR := os.Getenv("CLUSTER_SVC_CIDR")
	if svcCIDR == "" {
		svcCIDR = "10.43.0.0/16"
	}

	tcp := corev1.ProtocolTCP
	udp := corev1.ProtocolUDP
	externalPort := intstr.FromInt32(8080)
	internalPort := intstr.FromInt32(9090)
	dnsPort := intstr.FromInt32(53)

	desired := &networkingv1.NetworkPolicy{
		ObjectMeta: metav1.ObjectMeta{
			Name:      workspaceNetworkPolicyName,
			Namespace: nsName,
			Labels:    map[string]string{managedByLabel: managedByValue},
		},
		Spec: networkingv1.NetworkPolicySpec{
			PodSelector: metav1.LabelSelector{},
			PolicyTypes: []networkingv1.PolicyType{
				networkingv1.PolicyTypeIngress,
				networkingv1.PolicyTypeEgress,
			},
			Ingress: []networkingv1.NetworkPolicyIngressRule{
				// System namespace (operator) → both agent ports.
				{
					Ports: []networkingv1.NetworkPolicyPort{
						{Protocol: &tcp, Port: &externalPort},
						{Protocol: &tcp, Port: &internalPort},
					},
					From: []networkingv1.NetworkPolicyPeer{{
						NamespaceSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{"kubernetes.io/metadata.name": systemNS},
						},
					}},
				},
				// Gateway data-plane → external user-facing agent API only.
				{
					Ports: []networkingv1.NetworkPolicyPort{
						{Protocol: &tcp, Port: &externalPort},
					},
					From: []networkingv1.NetworkPolicyPeer{{
						NamespaceSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{"kubernetes.io/metadata.name": gatewayNS},
						},
					}},
				},
			},
			Egress: []networkingv1.NetworkPolicyEgressRule{
				// DNS resolution (kube-system CoreDNS).
				{
					Ports: []networkingv1.NetworkPolicyPort{
						{Protocol: &udp, Port: &dnsPort},
						{Protocol: &tcp, Port: &dnsPort},
					},
					To: []networkingv1.NetworkPolicyPeer{{
						NamespaceSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{"kubernetes.io/metadata.name": "kube-system"},
						},
					}},
				},
				// Gateway data-plane: the workspace reaches the in-cluster
				// registry (Zot), git (Gitea), and app by hostname, all routed
				// through the Envoy proxy — never by connecting to those pods
				// directly.
				{To: []networkingv1.NetworkPolicyPeer{{
					NamespaceSelector: &metav1.LabelSelector{
						MatchLabels: map[string]string{"kubernetes.io/metadata.name": gatewayNS},
					},
				}}},
				// Public internet, with the cluster's own pod/service CIDRs
				// carved out so a workspace cannot reach control-plane pods
				// (Postgres, authd, operator, app) at the network layer.
				{To: []networkingv1.NetworkPolicyPeer{{
					IPBlock: &networkingv1.IPBlock{
						CIDR:   "0.0.0.0/0",
						Except: []string{podCIDR, svcCIDR},
					},
				}}},
			},
		},
	}

	existing := &networkingv1.NetworkPolicy{}
	err := r.Get(ctx, types.NamespacedName{Namespace: nsName, Name: desired.Name}, existing)
	if apierrors.IsNotFound(err) {
		return r.Create(ctx, desired)
	}
	if err != nil {
		return err
	}
	existing.Spec = desired.Spec
	existing.Labels = desired.Labels
	return r.Update(ctx, existing)
}

// reconcileOrgRetention holds a soft-deleted org until its purge time, then
// hard-deletes its Projects and itself. The Project reaping triggers per-project
// cleanup; once the namespace is empty the finalizer removes the namespace.
func (r *OrganizationReconciler) reconcileOrgRetention(ctx context.Context, org *enzarbv1alpha1.Organization, nsName string, purgeTime time.Time) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	if time.Now().Before(purgeTime) {
		org.Status.Phase = "PendingDeletion"
		apimeta.SetStatusCondition(&org.Status.Conditions, metav1.Condition{
			Type:    "Ready",
			Status:  metav1.ConditionFalse,
			Reason:  "Retained",
			Message: fmt.Sprintf("soft-deleted; recoverable until %s", purgeTime.Format(time.RFC3339)),
		})
		if err := r.Status().Update(ctx, org); err != nil {
			return ctrl.Result{}, fmt.Errorf("update status: %w", err)
		}
		return ctrl.Result{RequeueAfter: time.Until(purgeTime)}, nil
	}

	// Purge time reached: delete remaining Projects, then the org itself.
	var projects enzarbv1alpha1.ProjectList
	if err := r.List(ctx, &projects, client.InNamespace(nsName)); err != nil {
		return ctrl.Result{}, fmt.Errorf("list projects: %w", err)
	}
	if len(projects.Items) > 0 {
		for i := range projects.Items {
			if projects.Items[i].DeletionTimestamp.IsZero() {
				if err := r.Delete(ctx, &projects.Items[i]); err != nil && !apierrors.IsNotFound(err) {
					return ctrl.Result{}, fmt.Errorf("purge project: %w", err)
				}
			}
		}
		logger.Info("purging org: deleting projects", "org", org.Spec.Slug, "count", len(projects.Items))
		return ctrl.Result{RequeueAfter: defaultRequeue}, nil
	}

	logger.Info("purging soft-deleted organization", "org", org.Spec.Slug)
	if err := r.Delete(ctx, org); err != nil && !apierrors.IsNotFound(err) {
		return ctrl.Result{}, fmt.Errorf("purge organization: %w", err)
	}
	return ctrl.Result{}, nil
}

func (r *OrganizationReconciler) reconcileDelete(ctx context.Context, org *enzarbv1alpha1.Organization, nsName string) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	if !controllerutil.ContainsFinalizer(org, organizationFinalizer) {
		return ctrl.Result{}, nil
	}

	// Refuse to tear down the namespace while Projects still exist in it.
	var projects enzarbv1alpha1.ProjectList
	if err := r.List(ctx, &projects, client.InNamespace(nsName)); err != nil {
		return ctrl.Result{}, fmt.Errorf("list projects: %w", err)
	}
	if len(projects.Items) > 0 {
		apimeta.SetStatusCondition(&org.Status.Conditions, metav1.Condition{
			Type:    "Ready",
			Status:  metav1.ConditionFalse,
			Reason:  "ProjectsRemain",
			Message: fmt.Sprintf("%d project(s) must be deleted before the organization", len(projects.Items)),
		})
		org.Status.Phase = "Terminating"
		if err := r.Status().Update(ctx, org); err != nil {
			return ctrl.Result{}, fmt.Errorf("update status: %w", err)
		}
		logger.Info("organization deletion blocked: projects remain", "org", org.Spec.Slug, "count", len(projects.Items))
		// Requeue so removing the last Project eventually unblocks teardown.
		return ctrl.Result{RequeueAfter: defaultRequeue}, nil
	}

	// Namespace is empty of Projects — delete it, then drop the finalizer.
	ns := &corev1.Namespace{}
	err := r.Get(ctx, types.NamespacedName{Name: nsName}, ns)
	switch {
	case err == nil:
		if ns.DeletionTimestamp.IsZero() {
			if err := r.Delete(ctx, ns); err != nil {
				return ctrl.Result{}, fmt.Errorf("delete namespace: %w", err)
			}
		}
	case apierrors.IsNotFound(err):
		// already gone
	default:
		return ctrl.Result{}, fmt.Errorf("get namespace: %w", err)
	}

	controllerutil.RemoveFinalizer(org, organizationFinalizer)
	if err := r.Update(ctx, org); err != nil {
		return ctrl.Result{}, fmt.Errorf("remove finalizer: %w", err)
	}
	logger.Info("organization deleted", "org", org.Spec.Slug, "namespace", nsName)
	return ctrl.Result{}, nil
}

func orgNamespaceLabels(org *enzarbv1alpha1.Organization) map[string]string {
	return map[string]string{
		"app.kubernetes.io/managed-by": "enzarb-operator",
		"enzarb.io/org-id":             org.Spec.OrgID,
		"enzarb.io/org-slug":           org.Spec.Slug,
	}
}
