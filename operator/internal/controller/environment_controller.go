package controller

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base32"
	"encoding/hex"
	goerrors "errors"
	"fmt"
	"net"
	"os"
	"strings"
	"time"

	acmev1 "github.com/cert-manager/cert-manager/pkg/apis/acme/v1"
	certmanagerv1 "github.com/cert-manager/cert-manager/pkg/apis/certmanager/v1"
	cmmeta "github.com/cert-manager/cert-manager/pkg/apis/meta/v1"
	corev1 "k8s.io/api/core/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"
	gatewayv1 "sigs.k8s.io/gateway-api/apis/v1"

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

	// Provision an ACME Issuer the tenant can reference (but not edit/delete) to
	// obtain TLS certs for their verified custom domains.
	if err := r.ensureEnvironmentIssuer(ctx, deployNS); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure environment issuer: %w", err)
	}

	// Deploy-namespace pods pull the project's private images from the in-cluster
	// registry with no imagePullSecret: the kubelet image credential provider
	// presents each pod's SA token to authd, which authorizes a pull-only scope
	// for this project via the namespace labels set in ensureNamespace.

	// Assign the environment its stable random serving subdomain before anything
	// derives hostnames from it.
	if _, err := ensureDeploySubdomain(&env); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deploy subdomain: %w", err)
	}

	// Verify ownership of custom domains and claim them in the cluster-scoped
	// ledger. This mutates env.Status.Domains in memory, which reconcileAllowedDomains
	// reads below, so it must run first.
	requeue, err := r.reconcileDomains(ctx, &project, &env)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("reconcile domains: %w", err)
	}

	if err := r.reconcileAllowedDomains(ctx, deployNS, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("reconcile allowed domains: %w", err)
	}

	// Issue per-domain certs into this namespace and provision the project's own
	// Gateway that references them locally (see reconcileServingTLS).
	if err := r.reconcileServingTLS(ctx, deployNS, servingDomains(&env)); err != nil {
		return ctrl.Result{}, fmt.Errorf("reconcile serving TLS: %w", err)
	}

	env.Status.Namespace = deployNS
	if err := r.Status().Update(ctx, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("update status: %w", err)
	}

	logger.Info("environment reconciled", "name", env.Name, "deployNS", deployNS)
	if requeue {
		// Re-poll DNS for domains still pending verification.
		return ctrl.Result{RequeueAfter: domainRecheckInterval}, nil
	}
	return ctrl.Result{}, nil
}

const (
	// domainRecheckInterval is how often the controller re-polls DNS for domains
	// that are not yet verified (and re-checks already-verified ones for drift).
	domainRecheckInterval = 2 * time.Minute
	// challengeLabel is the DNS label prefix under which tenants publish the
	// per-domain TXT proof, e.g. _enzarb-challenge.app.example.com.
	challengeLabel = "_enzarb-challenge"
	// challengePrefix prefixes the token in the TXT record value.
	challengePrefix = "enzarb-verify="
	// environmentIssuerName is the cert-manager Issuer provisioned in every deploy
	// namespace. Tenants reference it by name from their Certificates; they have no
	// RBAC to edit or delete it (see the enzarb-deployer ClusterRole).
	environmentIssuerName = "enzarb-acme"
)

// dnsResolver is package-level so tests can stub TXT lookups.
var dnsResolver interface {
	LookupTXT(ctx context.Context, name string) ([]string, error)
} = net.DefaultResolver

// reconcileDomains drives, for each custom domain, the ownership flow:
// generate a challenge token -> verify the TXT record -> claim the FQDN in the
// cluster-scoped DomainClaim ledger -> mark Verified. Only verified domains are
// projected into AllowedDomains (and thus admitted on routes), so a tenant can
// never serve a domain they have not proven DNS control of, nor one already
// claimed by another project. Returns requeue=true if any domain is still
// pending and should be re-polled.
func (r *EnvironmentReconciler) reconcileDomains(ctx context.Context, project *enzarbv1alpha1.Project, env *enzarbv1alpha1.Environment) (bool, error) {
	logger := log.FromContext(ctx)
	requeue := false

	// Prune status entries for domains no longer in spec.
	wanted := map[string]bool{}
	for _, cd := range env.Spec.CustomDomains {
		wanted[cd.FQDN] = true
	}
	kept := env.Status.Domains[:0]
	for _, ds := range env.Status.Domains {
		if wanted[ds.FQDN] {
			kept = append(kept, ds)
		}
	}
	env.Status.Domains = kept

	for _, cd := range env.Spec.CustomDomains {
		ds := getOrInitDomain(env, cd.FQDN)

		if ds.ChallengeToken == "" {
			tok, err := generateToken()
			if err != nil {
				return false, fmt.Errorf("generate token: %w", err)
			}
			ds.ChallengeToken = tok
			ds.CertStatus = "PendingVerification"
		}

		// Already verified and still owned by us: nothing to do (claim re-check is
		// cheap and guards against the claim being deleted out from under us).
		if ds.VerifiedAt != "" {
			setDomainStatus(env, cd.FQDN, *ds)
			continue
		}

		ok, err := verifyDomainTXT(ctx, cd.FQDN, ds.ChallengeToken)
		if err != nil {
			logger.Info("domain TXT lookup failed", "fqdn", cd.FQDN, "err", err.Error())
			ds.CertStatus = "VerificationError"
			setDomainStatus(env, cd.FQDN, *ds)
			requeue = true
			continue
		}
		if !ok {
			ds.CertStatus = "PendingVerification"
			setDomainStatus(env, cd.FQDN, *ds)
			requeue = true
			continue
		}

		// DNS proof succeeded; take the cluster-wide ownership lock.
		conflict, err := r.claimDomain(ctx, cd.FQDN, project, env)
		if err != nil {
			return false, fmt.Errorf("claim domain %s: %w", cd.FQDN, err)
		}
		if conflict {
			logger.Info("domain claimed by another project", "fqdn", cd.FQDN)
			ds.CertStatus = "DomainConflict"
			setDomainStatus(env, cd.FQDN, *ds)
			continue
		}

		ds.VerifiedAt = time.Now().UTC().Format(time.RFC3339)
		ds.CertStatus = "Verified"
		setDomainStatus(env, cd.FQDN, *ds)
		logger.Info("domain verified and claimed", "fqdn", cd.FQDN)
	}

	return requeue, nil
}

// getOrInitDomain returns a copy of the DomainStatus for fqdn, initializing one
// if absent. Callers persist changes via setDomainStatus.
func getOrInitDomain(env *enzarbv1alpha1.Environment, fqdn string) *enzarbv1alpha1.DomainStatus {
	for i := range env.Status.Domains {
		if env.Status.Domains[i].FQDN == fqdn {
			ds := env.Status.Domains[i]
			return &ds
		}
	}
	return &enzarbv1alpha1.DomainStatus{FQDN: fqdn}
}

func setDomainStatus(env *enzarbv1alpha1.Environment, fqdn string, ds enzarbv1alpha1.DomainStatus) {
	for i := range env.Status.Domains {
		if env.Status.Domains[i].FQDN == fqdn {
			env.Status.Domains[i] = ds
			return
		}
	}
	env.Status.Domains = append(env.Status.Domains, ds)
}

// verifyDomainTXT resolves _enzarb-challenge.<fqdn> and reports whether any TXT
// record carries the expected token, compared in constant time.
func verifyDomainTXT(ctx context.Context, fqdn, token string) (bool, error) {
	name := challengeLabel + "." + fqdn
	records, err := dnsResolver.LookupTXT(ctx, name)
	if err != nil {
		// NXDOMAIN / no records is "not yet verified", not a hard error.
		var dnsErr *net.DNSError
		if errorsAs(err, &dnsErr) && (dnsErr.IsNotFound || dnsErr.IsTemporary) {
			return false, nil
		}
		return false, err
	}
	want := []byte(challengePrefix + token)
	for _, rec := range records {
		if subtle.ConstantTimeCompare([]byte(rec), want) == 1 {
			return true, nil
		}
	}
	return false, nil
}

// errorsAs is a thin wrapper so the import stays local to this concern.
func errorsAs(err error, target any) bool { return goerrors.As(err, target) }

func generateToken() (string, error) {
	buf := make([]byte, 20)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	return base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(buf), nil
}

// claimName derives a DNS-1123-safe, collision-resistant DomainClaim object name
// from an FQDN. The sha256 hash is what gives etcd-level uniqueness.
func claimName(fqdn string) string {
	sum := sha256.Sum256([]byte(strings.ToLower(fqdn)))
	return "dc-" + hex.EncodeToString(sum[:])[:52]
}

// claimDomain atomically binds fqdn to this project. Returns conflict=true if the
// FQDN is already claimed by a different project. The Create is the lock: a racing
// second project's Create of the same hashed name fails with AlreadyExists.
func (r *EnvironmentReconciler) claimDomain(ctx context.Context, fqdn string, project *enzarbv1alpha1.Project, env *enzarbv1alpha1.Environment) (bool, error) {
	name := claimName(fqdn)
	existing := &enzarbv1alpha1.DomainClaim{}
	err := r.Get(ctx, types.NamespacedName{Name: name}, existing)
	if err == nil {
		owned := existing.Spec.OrgID == project.Spec.OrgID &&
			existing.Spec.ProjectRef == env.Spec.ProjectRef.Name &&
			existing.Spec.Namespace == env.Namespace &&
			existing.Spec.FQDN == fqdn
		return !owned, nil
	}
	if !errors.IsNotFound(err) {
		return false, err
	}

	claim := &enzarbv1alpha1.DomainClaim{
		ObjectMeta: metav1.ObjectMeta{
			Name: name,
			Labels: map[string]string{
				"app.kubernetes.io/managed-by": "enzarb-operator",
				"enzarb.io/org-id":             project.Spec.OrgID,
				"enzarb.io/project-slug":       project.Spec.Slug,
			},
		},
		Spec: enzarbv1alpha1.DomainClaimSpec{
			FQDN:       fqdn,
			OrgID:      project.Spec.OrgID,
			ProjectRef: env.Spec.ProjectRef.Name,
			Namespace:  env.Namespace,
		},
	}
	if err := r.Create(ctx, claim); err != nil {
		if errors.IsAlreadyExists(err) {
			// Lost the race; re-read to determine ownership.
			if gerr := r.Get(ctx, types.NamespacedName{Name: name}, existing); gerr != nil {
				return false, gerr
			}
			owned := existing.Spec.OrgID == project.Spec.OrgID &&
				existing.Spec.ProjectRef == env.Spec.ProjectRef.Name &&
				existing.Spec.Namespace == env.Namespace
			return !owned, nil
		}
		return false, err
	}
	claim.Status.VerifiedAt = time.Now().UTC().Format(time.RFC3339)
	if err := r.Status().Update(ctx, claim); err != nil {
		return false, err
	}
	return false, nil
}

// servingDomains is the ordered set of hostnames this environment serves: the
// deterministic platform subdomain (always, built from trusted CRD fields) plus
// every custom domain whose ownership has been DNS-verified. It is the single
// source of truth shared by AllowedDomains (admission), the per-domain
// Certificates, and the per-namespace Gateway listeners.
func servingDomains(env *enzarbv1alpha1.Environment) []string {
	verified := map[string]bool{}
	for _, d := range env.Status.Domains {
		if d.VerifiedAt != "" {
			verified[d.FQDN] = true
		}
	}
	// The platform host is a single random label under the deploy zone, so one
	// wildcard DNS record (*.<deploy zone>) covers every environment. Requires
	// the subdomain to have been generated (ensureDeploySubdomain) first.
	out := []string{}
	if env.Status.Subdomain != "" {
		out = append(out, env.Status.Subdomain+"."+deployZone())
	}
	for _, cd := range env.Spec.CustomDomains {
		if verified[cd.FQDN] {
			out = append(out, cd.FQDN)
		}
	}
	return out
}

// deployZone is the DNS zone under which environment serving hosts live. A single
// wildcard (*.<deployZone>) must point at the gateway. Defaults to apps.<base>.
func deployZone() string {
	if z := os.Getenv("DEPLOY_DOMAIN"); z != "" {
		return z
	}
	baseDomain := os.Getenv("BASE_DOMAIN")
	if baseDomain == "" {
		baseDomain = "enzarb.dev"
	}
	return "apps." + baseDomain
}

// ensureDeploySubdomain assigns the environment a stable random single DNS label
// the first time it is reconciled. The label is persisted in status and reused.
func ensureDeploySubdomain(env *enzarbv1alpha1.Environment) (bool, error) {
	if env.Status.Subdomain != "" {
		return false, nil
	}
	label, err := generateSubdomain()
	if err != nil {
		return false, err
	}
	env.Status.Subdomain = label
	return true, nil
}

// generateSubdomain returns a DNS-1123 label: a leading letter followed by
// lowercase base36 characters, with enough entropy that collisions across
// environments are negligible.
func generateSubdomain() (string, error) {
	const letters = "abcdefghijklmnopqrstuvwxyz"
	const alnum = "abcdefghijklmnopqrstuvwxyz0123456789"
	buf := make([]byte, 12)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	out := make([]byte, len(buf))
	out[0] = letters[int(buf[0])%len(letters)]
	for i := 1; i < len(buf); i++ {
		out[i] = alnum[int(buf[i])%len(alnum)]
	}
	return string(out), nil
}

// domainSecretName is the in-namespace TLS Secret (and Certificate) name for a
// serving domain. The cert lives in the deploy namespace and is referenced only
// by that namespace's own Gateway — never copied or injected elsewhere.
func domainSecretName(fqdn string) string {
	sum := sha256.Sum256([]byte(strings.ToLower(fqdn)))
	return "dtls-" + hex.EncodeToString(sum[:])[:48]
}

const deployGatewayName = "enzarb-deploy"

// reconcileServingTLS issues a TLS Certificate per serving domain into the deploy
// namespace (via the namespace-local enzarb-acme Issuer) and provisions the
// project's own Gateway whose listeners reference those local Secrets. Because
// the Gateway and the Secrets share the namespace, no ReferenceGrant is needed
// and no certificate material ever leaves the namespace. mergeGateways folds this
// Gateway into the shared Envoy/IP. Tenants hold no gateways RBAC, so they can
// route through this Gateway but cannot edit or delete it.
func (r *EnvironmentReconciler) reconcileServingTLS(ctx context.Context, deployNS string, domains []string) error {
	for _, d := range domains {
		if err := r.ensureDomainCertificate(ctx, deployNS, d); err != nil {
			return fmt.Errorf("ensure certificate %s: %w", d, err)
		}
	}
	return r.ensureDeployGateway(ctx, deployNS, domains)
}

func (r *EnvironmentReconciler) ensureDomainCertificate(ctx context.Context, deployNS, fqdn string) error {
	name := domainSecretName(fqdn)
	cert := &certmanagerv1.Certificate{}
	err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: name}, cert)
	if errors.IsNotFound(err) {
		cert = &certmanagerv1.Certificate{
			ObjectMeta: metav1.ObjectMeta{
				Name:      name,
				Namespace: deployNS,
				Labels:    map[string]string{"app.kubernetes.io/managed-by": "enzarb-operator"},
			},
			Spec: certmanagerv1.CertificateSpec{
				SecretName: name,
				DNSNames:   []string{fqdn},
				IssuerRef: cmmeta.IssuerReference{
					Name:  environmentIssuerName,
					Kind:  "Issuer",
					Group: "cert-manager.io",
				},
			},
		}
		return r.Create(ctx, cert)
	}
	return err
}

// ensureDeployGateway builds the namespace's enzarb-deploy Gateway with, per
// serving domain, an HTTPS listener (terminating with the domain's local cert
// Secret) and an HTTP listener (for the ACME HTTP-01 challenge and HTTP→HTTPS
// redirect). Listeners are keyed by unique hostname so they coexist on the merged
// Envoy across all projects.
func (r *EnvironmentReconciler) ensureDeployGateway(ctx context.Context, deployNS string, domains []string) error {
	className := os.Getenv("GATEWAY_CLASS_NAME")
	if className == "" {
		className = "envoy"
	}
	sameNS := gatewayv1.NamespacesFromSame
	tlsTerminate := gatewayv1.TLSModeTerminate

	var listeners []gatewayv1.Listener
	for _, d := range domains {
		host := gatewayv1.Hostname(d)
		secret := gatewayv1.ObjectName(domainSecretName(d))
		short := domainSecretName(d)[5:21] // stable, unique per-domain listener suffix
		listeners = append(listeners,
			gatewayv1.Listener{
				Name:     gatewayv1.SectionName("https-" + short),
				Port:     443,
				Protocol: gatewayv1.HTTPSProtocolType,
				Hostname: &host,
				TLS: &gatewayv1.ListenerTLSConfig{
					Mode:            &tlsTerminate,
					CertificateRefs: []gatewayv1.SecretObjectReference{{Name: secret}},
				},
				AllowedRoutes: &gatewayv1.AllowedRoutes{
					Namespaces: &gatewayv1.RouteNamespaces{From: &sameNS},
				},
			},
			gatewayv1.Listener{
				Name:     gatewayv1.SectionName("http-" + short),
				Port:     80,
				Protocol: gatewayv1.HTTPProtocolType,
				Hostname: &host,
				AllowedRoutes: &gatewayv1.AllowedRoutes{
					Namespaces: &gatewayv1.RouteNamespaces{From: &sameNS},
				},
			},
		)
	}

	gw := &gatewayv1.Gateway{}
	err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: deployGatewayName}, gw)
	if errors.IsNotFound(err) {
		gw = &gatewayv1.Gateway{
			ObjectMeta: metav1.ObjectMeta{
				Name:      deployGatewayName,
				Namespace: deployNS,
				Labels:    map[string]string{"app.kubernetes.io/managed-by": "enzarb-operator"},
			},
			Spec: gatewayv1.GatewaySpec{
				GatewayClassName: gatewayv1.ObjectName(className),
				Listeners:        listeners,
			},
		}
		return r.Create(ctx, gw)
	}
	if err != nil {
		return err
	}
	if !listenersEqual(gw.Spec.Listeners, listeners) {
		gw.Spec.Listeners = listeners
		return r.Update(ctx, gw)
	}
	return nil
}

func listenersEqual(a, b []gatewayv1.Listener) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i].Name != b[i].Name || a[i].Port != b[i].Port {
			return false
		}
		ah, bh := a[i].Hostname, b[i].Hostname
		if (ah == nil) != (bh == nil) || (ah != nil && *ah != *bh) {
			return false
		}
	}
	return true
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
func (r *EnvironmentReconciler) reconcileAllowedDomains(ctx context.Context, deployNS string, env *enzarbv1alpha1.Environment) error {
	fqdns := servingDomains(env)

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

// ensureEnvironmentIssuer provisions a namespaced ACME Issuer in the deploy
// namespace so tenants can issue TLS certs for their verified custom domains
// without any access to a ClusterIssuer. The Issuer is operator-owned: tenants
// hold no cert-manager "issuers" RBAC, so they can reference it from a
// Certificate (which needs no RBAC on the Issuer) but cannot edit or delete it.
//
// HTTP-01 is solved through the shared platform Gateway. The solver's HTTPRoute
// is created in this namespace for the (already DNS-verified, hence AllowedDomains-
// listed) hostname, so it passes the hostname admission policy. Each namespace
// registers its own ACME account via a generated per-namespace key secret.
func (r *EnvironmentReconciler) ensureEnvironmentIssuer(ctx context.Context, deployNS string) error {
	acmeServer := os.Getenv("ACME_SERVER")
	if acmeServer == "" {
		acmeServer = "https://acme-v02.api.letsencrypt.org/directory"
	}
	acmeEmail := os.Getenv("ACME_EMAIL")

	// Solve HTTP-01 on the project's own per-namespace Gateway (enzarb-deploy),
	// so the challenge route and the resulting cert Secret both stay in this
	// namespace. No Namespace on the parentRef -> same namespace as the Issuer.
	gwKind := gatewayv1.Kind("Gateway")
	gwGroup := gatewayv1.Group("gateway.networking.k8s.io")
	desired := acmev1.ACMEIssuer{
		Server: acmeServer,
		Email:  acmeEmail,
		PrivateKey: cmmeta.SecretKeySelector{
			LocalObjectReference: cmmeta.LocalObjectReference{Name: "enzarb-acme-account-key"},
		},
		Solvers: []acmev1.ACMEChallengeSolver{{
			HTTP01: &acmev1.ACMEChallengeSolverHTTP01{
				GatewayHTTPRoute: &acmev1.ACMEChallengeSolverHTTP01GatewayHTTPRoute{
					ParentRefs: []gatewayv1.ParentReference{{
						Name:  gatewayv1.ObjectName(deployGatewayName),
						Kind:  &gwKind,
						Group: &gwGroup,
					}},
				},
			},
		}},
	}

	issuer := &certmanagerv1.Issuer{}
	err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: environmentIssuerName}, issuer)
	if errors.IsNotFound(err) {
		issuer = &certmanagerv1.Issuer{
			ObjectMeta: metav1.ObjectMeta{
				Name:      environmentIssuerName,
				Namespace: deployNS,
				Labels: map[string]string{
					"app.kubernetes.io/managed-by": "enzarb-operator",
				},
			},
			Spec: certmanagerv1.IssuerSpec{
				IssuerConfig: certmanagerv1.IssuerConfig{ACME: &desired},
			},
		}
		return r.Create(ctx, issuer)
	}
	if err != nil {
		return err
	}
	// Reconcile drift so platform-level ACME/gateway changes propagate and any
	// tenant tampering (were it ever possible) is corrected.
	if issuer.Spec.ACME == nil || !acmeIssuerEqual(*issuer.Spec.ACME, desired) {
		issuer.Spec.IssuerConfig = certmanagerv1.IssuerConfig{ACME: &desired}
		return r.Update(ctx, issuer)
	}
	return nil
}

func acmeIssuerEqual(a, b acmev1.ACMEIssuer) bool {
	if a.Server != b.Server || a.Email != b.Email || a.PrivateKey.Name != b.PrivateKey.Name {
		return false
	}
	if len(a.Solvers) != len(b.Solvers) {
		return false
	}
	for i := range a.Solvers {
		ah, bh := a.Solvers[i].HTTP01, b.Solvers[i].HTTP01
		if (ah == nil) != (bh == nil) {
			return false
		}
		if ah == nil {
			continue
		}
		ag, bg := ah.GatewayHTTPRoute, bh.GatewayHTTPRoute
		if (ag == nil) != (bg == nil) {
			return false
		}
		if ag == nil {
			continue
		}
		if len(ag.ParentRefs) != len(bg.ParentRefs) {
			return false
		}
		for j := range ag.ParentRefs {
			if ag.ParentRefs[j].Name != bg.ParentRefs[j].Name {
				return false
			}
		}
	}
	return true
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
