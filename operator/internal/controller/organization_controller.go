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

	if err := r.ensureNetworkPolicy(ctx, &org, nsName); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure network policy: %w", err)
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

// ensureNetworkPolicy creates or updates the egress NetworkPolicy for the org
// namespace. Workspace pods may reach DNS, the k8s API server, enzarb-system
// services (gitea, zot), their own deploy namespaces, and the internet — but
// not other orgs' namespaces or other cluster services.
func (r *OrganizationReconciler) ensureNetworkPolicy(ctx context.Context, org *enzarbv1alpha1.Organization, ns string) error {
	if os.Getenv("NETWORK_POLICY_ENABLED") == "false" {
		return nil
	}

	podCIDR := os.Getenv("CLUSTER_POD_CIDR")
	svcCIDR := os.Getenv("CLUSTER_SVC_CIDR")
	if podCIDR == "" {
		podCIDR = "10.42.0.0/16"
	}
	if svcCIDR == "" {
		svcCIDR = "10.43.0.0/16"
	}

	dnsPort := intstr.FromInt32(53)
	apiserverPort := intstr.FromInt32(443)
	udp := corev1.ProtocolUDP
	tcp := corev1.ProtocolTCP

	// egress-only policy — ingress is unrestricted (the platform controls who
	// reaches workspace pods via HTTPRoute + agent JWT auth).
	desired := &networkingv1.NetworkPolicy{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "enzarb-workspace-egress",
			Namespace: ns,
			Labels: map[string]string{
				"app.kubernetes.io/managed-by": "enzarb-operator",
			},
		},
		Spec: networkingv1.NetworkPolicySpec{
			PodSelector: metav1.LabelSelector{},
			PolicyTypes: []networkingv1.PolicyType{networkingv1.PolicyTypeEgress},
			Egress: []networkingv1.NetworkPolicyEgressRule{
				// DNS — kube-dns in kube-system
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
				// Kubernetes API server (for kubectl in workspace)
				{
					Ports: []networkingv1.NetworkPolicyPort{
						{Protocol: &tcp, Port: &apiserverPort},
					},
					To: []networkingv1.NetworkPolicyPeer{{
						NamespaceSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{"kubernetes.io/metadata.name": "default"},
						},
					}},
				},
				// enzarb-system (gitea, zot, authd) — all ports
				{
					To: []networkingv1.NetworkPolicyPeer{{
						NamespaceSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{"kubernetes.io/metadata.name": "enzarb-system"},
						},
					}},
				},
				// Own deploy namespaces
				{
					To: []networkingv1.NetworkPolicyPeer{{
						NamespaceSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{
								"enzarb.io/org-id": org.Spec.OrgID,
								"enzarb.io/type":   "deploy",
							},
						},
					}},
				},
				// Internet egress — all IPs except cluster-internal CIDRs
				{
					To: []networkingv1.NetworkPolicyPeer{{
						IPBlock: &networkingv1.IPBlock{
							CIDR:   "0.0.0.0/0",
							Except: []string{podCIDR, svcCIDR},
						},
					}},
				},
			},
		},
	}

	existing := &networkingv1.NetworkPolicy{}
	err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: desired.Name}, existing)
	if apierrors.IsNotFound(err) {
		return r.Create(ctx, desired)
	}
	if err != nil {
		return err
	}
	existing.Spec = desired.Spec
	return r.Update(ctx, existing)
}

func orgNamespaceLabels(org *enzarbv1alpha1.Organization) map[string]string {
	return map[string]string{
		"app.kubernetes.io/managed-by": "enzarb-operator",
		"enzarb.io/org-id":             org.Spec.OrgID,
		"enzarb.io/org-slug":           org.Spec.Slug,
	}
}
