package controller

import (
	"context"
	"fmt"
	"os"

	corev1 "k8s.io/api/core/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
)

type EnvironmentReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

func (r *EnvironmentReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&enzarbv1alpha1.Environment{}).
		Complete(r)
}

func (r *EnvironmentReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var env enzarbv1alpha1.Environment
	if err := r.Get(ctx, req.NamespacedName, &env); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Resolve parent project to get org ID
	var project enzarbv1alpha1.Project
	if err := r.Get(ctx, types.NamespacedName{
		Namespace: env.Namespace,
		Name:      env.Spec.ProjectRef.Name,
	}, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("get project: %w", err)
	}

	// Namespace encodes org/project/env. The project slug isn't unambiguously
	// parseable back out (UUIDs and slugs both contain '-'), so authd resolves a
	// deploy pod's pull scope from the labels set below, not the name.
	deployNS := fmt.Sprintf("deploy-%s-%s-%s", project.Spec.OrgID, project.Spec.Slug, env.Spec.Slug)
	orgNS := env.Namespace
	saName := fmt.Sprintf("%s-sa", project.Spec.Slug)

	if err := r.ensureNamespace(ctx, deployNS, project.Spec.OrgID, project.Spec.Slug); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deploy namespace: %w", err)
	}

	if err := r.ensureDeployerRoleBinding(ctx, deployNS, orgNS, saName, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deployer rolebinding: %w", err)
	}

	// Deploy-namespace pods pull the project's private images from the in-cluster
	// registry with no imagePullSecret: the kubelet image credential provider
	// presents each pod's SA token to authd, which authorizes a pull-only scope
	// for this project via the namespace labels set in ensureNamespace.

	if err := r.reconcileAllowedDomains(ctx, deployNS, &project, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("reconcile allowed domains: %w", err)
	}

	env.Status.Namespace = deployNS
	if err := r.Status().Update(ctx, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("update status: %w", err)
	}

	logger.Info("environment reconciled", "name", env.Name, "deployNS", deployNS)
	return ctrl.Result{}, nil
}

// reconcileAllowedDomains projects the set of hostnames this environment is
// permitted to serve into an AllowedDomains object in the deploy namespace. The
// ValidatingAdmissionPolicy (charts/enzarb/templates/gateway-policy.yaml)
// paramRefs this object to reject tenant-authored HTTPRoute/GRPCRoute/Ingress
// resources whose hostnames fall outside the set, closing the domain-hijack
// vector for projects that deploy their own Gateway API resources.
//
// The set is: the deterministic platform subdomain (always allowed, derived
// from trusted CRD fields) plus any custom domain whose ownership has been DNS-
// verified (DomainStatus.VerifiedAt set). Unverified custom domains are
// deliberately omitted so a tenant cannot route a domain they haven't proven
// control of.
func (r *EnvironmentReconciler) reconcileAllowedDomains(ctx context.Context, deployNS string, project *enzarbv1alpha1.Project, env *enzarbv1alpha1.Environment) error {
	baseDomain := os.Getenv("BASE_DOMAIN")
	if baseDomain == "" {
		baseDomain = "enzarb.dev"
	}

	// Deterministic platform hostname built only from trusted CRD fields, so it
	// is collision-free by construction and never derived from user input.
	platformHost := fmt.Sprintf("%s.%s.%s", env.Spec.Slug, project.Spec.Slug, baseDomain)

	verified := map[string]bool{}
	for _, d := range env.Status.Domains {
		if d.VerifiedAt != "" {
			verified[d.FQDN] = true
		}
	}

	fqdns := []string{platformHost}
	for _, cd := range env.Spec.CustomDomains {
		if verified[cd.FQDN] {
			fqdns = append(fqdns, cd.FQDN)
		}
	}

	ad := &enzarbv1alpha1.AllowedDomains{}
	err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: "default"}, ad)
	if errors.IsNotFound(err) {
		ad = &enzarbv1alpha1.AllowedDomains{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "default",
				Namespace: deployNS,
				Labels: map[string]string{
					"app.kubernetes.io/managed-by": "enzarb-operator",
				},
			},
			Spec: enzarbv1alpha1.AllowedDomainsSpec{FQDNs: fqdns},
		}
		return r.Create(ctx, ad)
	}
	if err != nil {
		return err
	}
	if !equalStrings(ad.Spec.FQDNs, fqdns) {
		ad.Spec.FQDNs = fqdns
		return r.Update(ctx, ad)
	}
	return nil
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func (r *EnvironmentReconciler) ensureDeployerRoleBinding(ctx context.Context, deployNS, orgNS, saName string, env *enzarbv1alpha1.Environment) error {
	name := fmt.Sprintf("enzarb-deployer-%s", env.Spec.ProjectRef.Name)
	rb := &rbacv1.RoleBinding{}
	err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: name}, rb)
	if errors.IsNotFound(err) {
		rb = &rbacv1.RoleBinding{
			ObjectMeta: metav1.ObjectMeta{
				Name:      name,
				Namespace: deployNS,
			},
			RoleRef: rbacv1.RoleRef{
				APIGroup: "rbac.authorization.k8s.io",
				Kind:     "ClusterRole",
				Name:     "enzarb-deployer",
			},
			Subjects: []rbacv1.Subject{
				{Kind: "ServiceAccount", Name: saName, Namespace: orgNS},
			},
		}
		return r.Create(ctx, rb)
	}
	return err
}

func (r *EnvironmentReconciler) ensureNamespace(ctx context.Context, name, orgID, projectSlug string) error {
	// authd reads enzarb.io/org-id and enzarb.io/project-slug to authorize a
	// deploy pod's pull-only registry scope (see authd deployIdentity).
	labels := map[string]string{
		"app.kubernetes.io/managed-by": "enzarb-operator",
		"enzarb.io/type":               "deploy",
		"enzarb.io/org-id":             orgID,
		"enzarb.io/project-slug":       projectSlug,
	}
	ns := &corev1.Namespace{}
	err := r.Get(ctx, types.NamespacedName{Name: name}, ns)
	if errors.IsNotFound(err) {
		return r.Create(ctx, &corev1.Namespace{
			ObjectMeta: metav1.ObjectMeta{
				Name:   name,
				Labels: labels,
			},
		})
	}
	if err != nil {
		return err
	}
	// Backfill labels on a pre-existing namespace so authd can resolve scope.
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
