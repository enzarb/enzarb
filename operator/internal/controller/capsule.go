package controller

// Capsule / capsule-proxy integration (deploy/system/capsule.yaml).
//
// Workspace pods cannot be granted `list namespaces` without seeing every
// tenant's namespaces, so their kubectl is pointed at capsule-proxy, which
// filters cluster-scoped list responses down to the Tenants the requesting
// ServiceAccount owns. The operator maintains:
//   - one Capsule Tenant per Project (owner = workspace SA, bound to the
//     enzarb-deployer ClusterRole instead of Capsule's default admin)
//   - the capsule.clastix.io/tenant label on each deploy namespace (set in
//     ensureNamespace) so Capsule attributes it to the Tenant
//   - the org's ServiceAccount group in CapsuleConfiguration's user scope
//     (Capsule ignores identities outside that scope, which also keeps the
//     operator's own namespace management out of Capsule's webhooks)
//   - a workspace kubeconfig ConfigMap pointing kubectl at the proxy
//
// Everything here tolerates Capsule not being installed (NoMatch/NotFound):
// the workspace then falls back to talking to the API server directly.

import (
	"context"
	"fmt"
	"reflect"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	apimeta "k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/apis/meta/v1/unstructured"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/log"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
)

const (
	capsuleSystemNamespace = "capsule-system"
	// capsuleProxyCASecretName is the cert-manager-generated self-signed CA
	// behind capsule-proxy's serving cert (chart certManager.generateCertificates).
	capsuleProxyCASecretName = "capsule-proxy-root-secret"
	capsuleProxyServer       = "https://capsule-proxy.capsule-system.svc:9001"
	capsuleTenantLabel       = "capsule.clastix.io/tenant"
	// workspaceKubeconfigPath is where the kubeconfig ConfigMap is mounted in
	// the workspace container; KUBECONFIG points at the "config" key.
	workspaceKubeconfigMountPath = "/var/run/enzarb/kubeconfig"
	serviceAccountTokenFile      = "/var/run/secrets/kubernetes.io/serviceaccount/token" //nolint:gosec // path, not a credential
	serviceAccountCAFile         = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt"
)

var (
	capsuleTenantGVK = schema.GroupVersionKind{Group: "capsule.clastix.io", Version: "v1beta2", Kind: "Tenant"}
	capsuleConfigGVK = schema.GroupVersionKind{Group: "capsule.clastix.io", Version: "v1beta2", Kind: "CapsuleConfiguration"}
)

// capsuleTenantName is the cluster-scoped Tenant name for a project.
func capsuleTenantName(orgID, slug string) string {
	return fmt.Sprintf("enzarb-%s-%s", orgID, slug)
}

// capsuleAbsent reports errors that just mean Capsule isn't installed (yet).
func capsuleAbsent(err error) bool {
	return apimeta.IsNoMatchError(err) || errors.IsNotFound(err)
}

// orgServiceAccountsGroup is the Kubernetes group covering every SA in the
// org namespace — this is what enters Capsule's user scope, so only workspace
// SAs (which live there) are treated as tenant subjects.
func orgServiceAccountsGroup(orgID string) string {
	return "system:serviceaccounts:" + orgNamespaceName(orgID)
}

// ensureCapsuleTenant maintains the project's Tenant with the workspace SA as
// owner. The owner is bound to enzarb-deployer rather than Capsule's default
// (admin + capsule-namespace-deleter), so Capsule grants nothing beyond what
// the Environment reconciler's RoleBinding already grants.
func (r *ProjectReconciler) ensureCapsuleTenant(ctx context.Context, orgNS, saName string, project *enzarbv1alpha1.Project) error {
	name := capsuleTenantName(project.Spec.OrgID, project.Spec.Slug)
	desiredOwners := []any{map[string]any{
		"kind":         "ServiceAccount",
		"name":         fmt.Sprintf("system:serviceaccount:%s:%s", orgNS, saName),
		"clusterRoles": []any{"enzarb-deployer"},
	}}

	tenant := &unstructured.Unstructured{}
	tenant.SetGroupVersionKind(capsuleTenantGVK)
	err := r.Get(ctx, types.NamespacedName{Name: name}, tenant)
	if apimeta.IsNoMatchError(err) {
		log.FromContext(ctx).V(1).Info("capsule not installed; skipping tenant", "tenant", name)
		return nil
	}
	if errors.IsNotFound(err) {
		tenant = &unstructured.Unstructured{}
		tenant.SetGroupVersionKind(capsuleTenantGVK)
		tenant.SetName(name)
		tenant.SetLabels(projectLabels(project))
		if err := unstructured.SetNestedSlice(tenant.Object, desiredOwners, "spec", "owners"); err != nil {
			return err
		}
		return r.Create(ctx, tenant)
	}
	if err != nil {
		return err
	}
	currentOwners, _, _ := unstructured.NestedSlice(tenant.Object, "spec", "owners")
	if !reflect.DeepEqual(currentOwners, desiredOwners) {
		if err := unstructured.SetNestedSlice(tenant.Object, desiredOwners, "spec", "owners"); err != nil {
			return err
		}
		return r.Update(ctx, tenant)
	}
	return nil
}

// deleteCapsuleTenant removes the project's Tenant. Capsule's ownerReference
// on adopted deploy namespaces cascades their deletion, which is the desired
// end state for a hard-deleted project.
func (r *ProjectReconciler) deleteCapsuleTenant(ctx context.Context, project *enzarbv1alpha1.Project) error {
	tenant := &unstructured.Unstructured{}
	tenant.SetGroupVersionKind(capsuleTenantGVK)
	tenant.SetName(capsuleTenantName(project.Spec.OrgID, project.Spec.Slug))
	if err := r.Delete(ctx, tenant); err != nil && !capsuleAbsent(err) {
		return err
	}
	return nil
}

// reconcileCapsuleUserGroup adds (or removes, on org deletion) the org's
// ServiceAccount group in the CapsuleConfiguration user scope. Capsule only
// treats identities inside that scope as tenant subjects — required for the
// proxy to resolve workspace SAs to their Tenants.
func (r *OrganizationReconciler) reconcileCapsuleUserGroup(ctx context.Context, orgID string, remove bool) error {
	cfg := &unstructured.Unstructured{}
	cfg.SetGroupVersionKind(capsuleConfigGVK)
	err := r.Get(ctx, types.NamespacedName{Name: "default"}, cfg)
	if capsuleAbsent(err) {
		log.FromContext(ctx).V(1).Info("capsule not installed; skipping user group", "org", orgID)
		return nil
	}
	if err != nil {
		return err
	}
	group := orgServiceAccountsGroup(orgID)
	groups, _, _ := unstructured.NestedStringSlice(cfg.Object, "spec", "userGroups")
	out := make([]string, 0, len(groups)+1)
	present := false
	for _, g := range groups {
		if g == group {
			present = true
			if remove {
				continue
			}
		}
		out = append(out, g)
	}
	if !remove && !present {
		out = append(out, group)
	}
	if (present && remove) || (!present && !remove) {
		if err := unstructured.SetNestedStringSlice(cfg.Object, out, "spec", "userGroups"); err != nil {
			return err
		}
		return r.Update(ctx, cfg)
	}
	return nil
}

// ensureWorkspaceKubeconfig maintains the "<slug>-kubeconfig" ConfigMap
// mounted into the workspace pod (KUBECONFIG points at its "config" key).
// When the capsule-proxy CA is available the kubeconfig routes kubectl
// through the proxy so cluster-scoped lists are tenant-filtered; otherwise it
// reproduces the plain in-cluster configuration so behavior is unchanged.
func (r *ProjectReconciler) ensureWorkspaceKubeconfig(ctx context.Context, ns string, project *enzarbv1alpha1.Project) error {
	server := "https://kubernetes.default.svc"
	caFile := serviceAccountCAFile
	caPEM := ""

	secret := &corev1.Secret{}
	err := r.Get(ctx, types.NamespacedName{Namespace: capsuleSystemNamespace, Name: capsuleProxyCASecretName}, secret)
	switch {
	case err == nil:
		if ca := secret.Data["ca.crt"]; len(ca) > 0 {
			caPEM = string(ca)
		} else if ca := secret.Data["tls.crt"]; len(ca) > 0 {
			caPEM = string(ca)
		}
		if caPEM != "" {
			server = capsuleProxyServer
			caFile = workspaceKubeconfigMountPath + "/proxy-ca.crt"
		}
	case errors.IsNotFound(err) || errors.IsForbidden(err):
		// capsule-proxy not deployed (or RBAC not rolled out yet): fall back
		// to the direct API server config.
	default:
		return fmt.Errorf("get capsule-proxy CA: %w", err)
	}

	kubeconfig := fmt.Sprintf(`apiVersion: v1
kind: Config
clusters:
- name: enzarb
  cluster:
    server: %s
    certificate-authority: %s
contexts:
- name: enzarb
  context:
    cluster: enzarb
    user: workspace
current-context: enzarb
users:
- name: workspace
  user:
    tokenFile: %s
`, server, caFile, serviceAccountTokenFile)

	data := map[string]string{"config": kubeconfig}
	if caPEM != "" {
		data["proxy-ca.crt"] = caPEM
	}

	cmName := workspaceKubeconfigName(project.Spec.Slug)
	cm := &corev1.ConfigMap{}
	err = r.Get(ctx, types.NamespacedName{Namespace: ns, Name: cmName}, cm)
	if errors.IsNotFound(err) {
		cm = &corev1.ConfigMap{
			ObjectMeta: metav1.ObjectMeta{
				Name:      cmName,
				Namespace: ns,
				Labels:    projectLabels(project),
			},
			Data: data,
		}
		if err := controllerutil.SetControllerReference(project, cm, r.Scheme); err != nil {
			return err
		}
		return r.Create(ctx, cm)
	}
	if err != nil {
		return err
	}
	if reflect.DeepEqual(cm.Data, data) {
		return nil
	}
	cm.Data = data
	return r.Update(ctx, cm)
}

func workspaceKubeconfigName(slug string) string {
	return fmt.Sprintf("%s-kubeconfig", slug)
}
