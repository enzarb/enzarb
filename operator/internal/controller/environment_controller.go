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
	networkingv1 "k8s.io/api/networking/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/intstr"
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

	// Namespace name is sticky once assigned: reuse the existing one so
	// environments created before deployNamespaceName existed keep their
	// original (unhashed) namespace instead of getting orphaned by a rename.
	// New environments get a truncated, human-readable prefix plus a hash
	// suffix (see deployNamespaceName) since org UUID + two 63-char slugs can
	// exceed the Kubernetes 63-char namespace limit. Either way the name isn't
	// unambiguously parseable back out, so authd/metering resolve org/project/env
	// from the labels set below.
	deployNS := env.Status.Namespace
	if deployNS == "" {
		deployNS = deployNamespaceName(project.Spec.OrgID, project.Spec.Slug, env.Spec.Slug)
	}
	orgNS := env.Namespace
	saName := fmt.Sprintf("%s-sa", project.Spec.Slug)

	if err := r.ensureNamespace(ctx, deployNS, project.Spec.OrgID, project.Spec.Slug, env.Spec.Slug); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deploy namespace: %w", err)
	}

	if err := r.ensureDeployerRoleBinding(ctx, deployNS, orgNS, saName, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deployer rolebinding: %w", err)
	}

	if err := r.ensureNetworkPolicy(ctx, deployNS, orgNS, project.Spec.Slug); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure network policy: %w", err)
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

	// Honour a user-initiated "recheck now" request: any annotation change
	// already triggers this reconcile via the watch, so all that's needed here
	// is to clear the one-shot flag before reconcileDomains runs its (always
	// unconditional) verification pass below.
	if env.Annotations[recheckDomainsAnnotation] == "true" {
		patch := []byte(`{"metadata":{"annotations":{"` + recheckDomainsAnnotation + `":null}}}`)
		if pErr := r.Patch(ctx, &env, client.RawPatch(types.MergePatchType, patch)); pErr != nil {
			logger.Error(pErr, "failed to remove recheck-domains annotation; proceeding anyway")
		}
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

	// Surface cert-manager's issuance progress for each verified custom domain
	// so the UI can show more than just "Verified" while the cert/gateway
	// catch up.
	if err := r.reconcileDomainTLSStatus(ctx, deployNS, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("reconcile domain TLS status: %w", err)
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
	// recheckDomainsAnnotation lets a tenant request an immediate domain
	// verification pass instead of waiting for domainRecheckInterval. Setting it
	// to "true" triggers a reconcile via the watch; Reconcile clears it as a
	// one-shot flag before reconcileDomains runs.
	recheckDomainsAnnotation = "enzarb.io/recheck-domains"
)

// dnsResolver is package-level so tests can stub TXT lookups.
var dnsResolver interface {
	LookupTXT(ctx context.Context, name string) ([]string, error)
} = net.DefaultResolver

// hostResolver is package-level so tests can stub A/AAAA lookups for the
// routing check.
var hostResolver interface {
	LookupHost(ctx context.Context, host string) ([]string, error)
} = net.DefaultResolver

// gatewayPublicIPs returns the deploy gateway's Service's spec.externalIPs —
// the router-facing address(es) customer domains must resolve to. Set via
// the EnvoyProxy envoyService patch (charts/enzarb/templates/gateway.yaml),
// itself driven by the single gateway.publicIP Helm value, so this is the one
// place that value is read from rather than a second, possibly-stale copy.
func (r *EnvironmentReconciler) gatewayPublicIPs(ctx context.Context) ([]string, error) {
	gatewayNS := os.Getenv("GATEWAY_NAMESPACE")
	className := os.Getenv("GATEWAY_CLASS_NAME")
	if className == "" {
		className = "envoy"
	}
	var svcs corev1.ServiceList
	if err := r.List(ctx, &svcs, client.InNamespace(gatewayNS),
		client.MatchingLabels{"gateway.envoyproxy.io/owning-gatewayclass": className}); err != nil {
		return nil, err
	}
	var ips []string
	for _, svc := range svcs.Items {
		ips = append(ips, svc.Spec.ExternalIPs...)
	}
	return ips, nil
}

// reconcileDomainRouting checks whether fqdn's public A/AAAA records actually
// resolve to the gateway's public IP(s), and updates ds.RoutingStatus/
// RoutingError accordingly. Returns true once routing is Ready (i.e. no
// further requeue needed for this reason).
func (r *EnvironmentReconciler) reconcileDomainRouting(ctx context.Context, ds *enzarbv1alpha1.DomainStatus) bool {
	logger := log.FromContext(ctx)

	publicIPs, err := r.gatewayPublicIPs(ctx)
	if err != nil {
		ds.RoutingStatus = "Error"
		ds.RoutingError = err.Error()
		return false
	}
	if len(publicIPs) == 0 {
		// Not configured yet; don't block on a routing check we can't perform.
		ds.RoutingStatus = "Pending"
		ds.RoutingError = ""
		return false
	}

	want := map[string]bool{}
	for _, ip := range publicIPs {
		want[ip] = true
	}

	resolved, err := hostResolver.LookupHost(ctx, ds.FQDN)
	if err != nil {
		var dnsErr *net.DNSError
		if errorsAs(err, &dnsErr) && (dnsErr.IsNotFound || dnsErr.IsTemporary) {
			ds.RoutingStatus = "Pending"
			ds.RoutingError = ""
			return false
		}
		logger.Info("domain routing lookup failed", "fqdn", ds.FQDN, "err", err.Error())
		ds.RoutingStatus = "Error"
		ds.RoutingError = err.Error()
		return false
	}

	for _, ip := range resolved {
		if want[ip] {
			ds.RoutingStatus = "Ready"
			ds.RoutingError = ""
			return true
		}
	}
	ds.RoutingStatus = "Pending"
	ds.RoutingError = ""
	return false
}

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

		// Already verified and still owned by us: skip re-proving ownership, but
		// still re-check routing (claim re-check is cheap and guards against the
		// claim being deleted out from under us).
		if ds.VerifiedAt != "" {
			if !r.reconcileDomainRouting(ctx, ds) {
				requeue = true
			}
			ds.LastCheckedAt = time.Now().UTC().Format(time.RFC3339)
			setDomainStatus(env, cd.FQDN, *ds)
			continue
		}

		ok, err := verifyDomainTXT(ctx, cd.FQDN, ds.ChallengeToken)
		ds.LastCheckedAt = time.Now().UTC().Format(time.RFC3339)
		if err != nil {
			logger.Info("domain TXT lookup failed", "fqdn", cd.FQDN, "err", err.Error())
			ds.CertStatus = "VerificationError"
			ds.LastError = err.Error()
			setDomainStatus(env, cd.FQDN, *ds)
			requeue = true
			continue
		}
		if !ok {
			ds.CertStatus = "PendingVerification"
			ds.LastError = ""
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
		ds.LastError = ""
		if !r.reconcileDomainRouting(ctx, ds) {
			requeue = true
		}
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

const maxNamespaceNameLen = 63

// deployNamespaceName builds the deploy namespace name for an environment. The
// org UUID (36 chars) plus two independently-validated 63-char slugs can exceed
// Kubernetes' 63-char namespace limit, so this truncates the human-readable
// project/env slugs and appends a hash of the full (untruncated) identity for
// uniqueness. The name is not meant to be parsed back apart — org/project/env
// identity lives in the namespace labels set by ensureNamespace.
func deployNamespaceName(orgID, projectSlug, envSlug string) string {
	const prefix = "deploy-"
	sum := sha256.Sum256([]byte(orgID + "/" + projectSlug + "/" + envSlug))
	hash := hex.EncodeToString(sum[:])[:10]

	budget := maxNamespaceNameLen - len(prefix) - len(hash) - 1 // -1 for the dash before the readable part
	readable := truncateLabel(projectSlug+"-"+envSlug, budget)
	if readable == "" {
		return prefix + hash
	}
	return prefix + hash + "-" + readable
}

// truncateLabel cuts s to at most n bytes, trimming any trailing '-' left
// dangling by the cut so the result stays a valid DNS-1123 label segment.
func truncateLabel(s string, n int) string {
	if n <= 0 {
		return ""
	}
	if len(s) > n {
		s = s[:n]
	}
	return strings.TrimRight(s, "-")
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
	// A cert is requested only once ownership is proven AND the domain
	// actually routes to us — TXT ownership alone doesn't mean production
	// traffic can reach the gateway, and requesting an ACME cert for a domain
	// that isn't routed yet just burns Let's Encrypt's rate limit on a
	// guaranteed-fail HTTP-01 validation.
	ready := map[string]bool{}
	for _, d := range env.Status.Domains {
		if d.VerifiedAt != "" && d.RoutingStatus == "Ready" {
			ready[d.FQDN] = true
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
		if ready[cd.FQDN] {
			out = append(out, cd.FQDN)
		}
	}
	return out
}

// deployZone is the DNS zone under which environment serving hosts live. A single
// wildcard (*.<deployZone>) must point at the gateway. Defaults to env.<base>.
func deployZone() string {
	if z := os.Getenv("DEPLOY_DOMAIN"); z != "" {
		return z
	}
	baseDomain := os.Getenv("BASE_DOMAIN")
	if baseDomain == "" {
		baseDomain = "enzarb.dev"
	}
	return "env." + baseDomain
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

// servingCertLabel marks the per-domain TLS Certificates the operator manages in
// a deploy namespace, so stale ones can be pruned without touching tenant certs.
const servingCertLabel = "enzarb.io/serving-cert"

// reconcileServingTLS issues a TLS Certificate per serving domain into the deploy
// namespace (via the namespace-local enzarb-acme Issuer) and provisions the
// project's own Gateway whose listeners reference those local Secrets. Because
// the Gateway and the Secrets share the namespace, no ReferenceGrant is needed
// and no certificate material ever leaves the namespace. mergeGateways folds this
// Gateway into the shared Envoy/IP. Tenants hold no gateways RBAC, so they can
// route through this Gateway but cannot edit or delete it.
func (r *EnvironmentReconciler) reconcileServingTLS(ctx context.Context, deployNS string, domains []string) error {
	desired := map[string]bool{}
	for _, d := range domains {
		desired[domainSecretName(d)] = true
		if err := r.ensureDomainCertificate(ctx, deployNS, d); err != nil {
			return fmt.Errorf("ensure certificate %s: %w", d, err)
		}
	}
	if err := r.ensureDeployGateway(ctx, deployNS, domains); err != nil {
		return err
	}
	return r.pruneServingCertificates(ctx, deployNS, desired)
}

// pruneServingCertificates deletes operator-managed serving Certificates in the
// deploy namespace that are no longer in the desired set — e.g. left behind when
// an environment's serving host changes or a custom domain is removed. Only certs
// carrying servingCertLabel are considered, so tenant-created certs are untouched.
// cert-manager garbage-collects the backing Secret once the Certificate is gone.
func (r *EnvironmentReconciler) pruneServingCertificates(ctx context.Context, deployNS string, desired map[string]bool) error {
	var certs certmanagerv1.CertificateList
	if err := r.List(ctx, &certs, client.InNamespace(deployNS),
		client.MatchingLabels{"app.kubernetes.io/managed-by": "enzarb-operator"}); err != nil {
		return err
	}
	logger := log.FromContext(ctx)
	for i := range certs.Items {
		c := &certs.Items[i]
		// Serving certs are exactly the operator-managed ones named dtls-<hash>.
		if desired[c.Name] || !strings.HasPrefix(c.Name, "dtls-") {
			continue
		}
		if err := r.Delete(ctx, c); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("prune certificate %s: %w", c.Name, err)
		}
		logger.Info("pruned stale serving certificate", "namespace", deployNS, "name", c.Name)
	}
	return nil
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
				Labels: map[string]string{
					"app.kubernetes.io/managed-by": "enzarb-operator",
					servingCertLabel:               "true",
				},
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

// reconcileDomainTLSStatus reads back the cert-manager Certificate created by
// ensureDomainCertificate for each verified custom domain and projects its
// Ready condition into DomainStatus.TLSStatus/TLSError, so the UI has
// visibility past "Verified" into whether the gateway is actually serving the
// domain yet.
func (r *EnvironmentReconciler) reconcileDomainTLSStatus(ctx context.Context, deployNS string, env *enzarbv1alpha1.Environment) error {
	for i := range env.Status.Domains {
		ds := &env.Status.Domains[i]
		if ds.CertStatus != "Verified" || ds.RoutingStatus != "Ready" {
			ds.TLSStatus = ""
			ds.TLSError = ""
			continue
		}

		cert := &certmanagerv1.Certificate{}
		name := domainSecretName(ds.FQDN)
		if err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: name}, cert); err != nil {
			if errors.IsNotFound(err) {
				ds.TLSStatus = "IssuingCertificate"
				ds.TLSError = ""
				continue
			}
			return fmt.Errorf("get certificate %s: %w", name, err)
		}

		ready := certReadyCondition(cert)
		switch {
		case ready != nil && ready.Status == cmmeta.ConditionTrue:
			ds.TLSStatus = "Ready"
			ds.TLSError = ""
		case ready != nil && ready.Status == cmmeta.ConditionFalse:
			ds.TLSStatus = "CertError"
			ds.TLSError = ready.Message
		default:
			ds.TLSStatus = "IssuingCertificate"
			ds.TLSError = ""
		}
	}
	return nil
}

func certReadyCondition(cert *certmanagerv1.Certificate) *certmanagerv1.CertificateCondition {
	for i := range cert.Status.Conditions {
		if cert.Status.Conditions[i].Type == certmanagerv1.CertificateConditionReady {
			return &cert.Status.Conditions[i]
		}
	}
	return nil
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

// ensureNetworkPolicy creates the ingress + egress NetworkPolicies for a deploy
// namespace that enforce tenant isolation:
//   - Ingress: only the Envoy gateway namespace, the owning project's workspace
//     pods, and same-namespace pods may reach deploy pods. Other deploy
//     namespaces (even of the same project) cannot.
//   - Egress: DNS, internet (excluding cluster CIDRs), and same-namespace. No
//     direct access to other cluster namespaces, including other org/deploy ns.
func (r *EnvironmentReconciler) ensureNetworkPolicy(ctx context.Context, deployNS, orgNS, projectSlug string) error {
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

	// Namespace where Envoy Gateway runs its data-plane proxy pods. The proxy
	// terminating gateway traffic lives here (not in the deploy namespace), so
	// the ingress rule must admit it or all routed traffic — including the ACME
	// HTTP-01 self-check — is dropped.
	gatewayNS := os.Getenv("GATEWAY_NAMESPACE")
	if gatewayNS == "" {
		gatewayNS = "envoy-gateway-system"
	}

	// Namespace where the pgop Postgres operator runs. Its controllers must open
	// a TCP connection to the tenant Postgres pod (:5432) to reconcile Role and
	// Database resources. Without this exception Cilium drops the connection and
	// all pgop reconciliation fails indefinitely.
	pgopNS := os.Getenv("PGOP_NAMESPACE")
	if pgopNS == "" {
		pgopNS = "pgop-system"
	}

	dnsPort := intstr.FromInt32(53)
	pgPort := intstr.FromInt32(5432)
	udp := corev1.ProtocolUDP
	tcp := corev1.ProtocolTCP

	desired := &networkingv1.NetworkPolicy{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "enzarb-deploy-isolation",
			Namespace: deployNS,
			Labels: map[string]string{
				"app.kubernetes.io/managed-by": "enzarb-operator",
			},
		},
		Spec: networkingv1.NetworkPolicySpec{
			PodSelector: metav1.LabelSelector{},
			PolicyTypes: []networkingv1.PolicyType{
				networkingv1.PolicyTypeIngress,
				networkingv1.PolicyTypeEgress,
			},
			Ingress: []networkingv1.NetworkPolicyIngressRule{
				// Same-namespace service-to-service within this env, plus the Envoy
				// Gateway proxy namespace (the merged data-plane proxy that routes
				// all gateway traffic, including the ACME HTTP-01 challenge solver,
				// runs there rather than in the deploy namespace).
				{From: []networkingv1.NetworkPolicyPeer{
					{PodSelector: &metav1.LabelSelector{}},
					{NamespaceSelector: &metav1.LabelSelector{
						MatchLabels: map[string]string{"kubernetes.io/metadata.name": gatewayNS},
					}},
					// The owning project's workspace pod (not the whole org
					// namespace), so a workspace can reach the services it
					// deploys here.
					{
						NamespaceSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{"kubernetes.io/metadata.name": orgNS},
						},
						PodSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{"enzarb.io/project": projectSlug},
						},
					},
				}},
				// pgop operator ingress: the controller connects to the tenant Postgres
				// pod on 5432 to reconcile Role and Database CRs.
				{
					Ports: []networkingv1.NetworkPolicyPort{
						{Protocol: &tcp, Port: &pgPort},
					},
					From: []networkingv1.NetworkPolicyPeer{{
						NamespaceSelector: &metav1.LabelSelector{
							MatchLabels: map[string]string{"kubernetes.io/metadata.name": pgopNS},
						},
					}},
				},
			},
			Egress: []networkingv1.NetworkPolicyEgressRule{
				// DNS
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
				// Same-namespace service-to-service
				{To: []networkingv1.NetworkPolicyPeer{{
					PodSelector: &metav1.LabelSelector{},
				}}},
				// Internet egress (excluding cluster-internal CIDRs)
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
	err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: desired.Name}, existing)
	if errors.IsNotFound(err) {
		return r.Create(ctx, desired)
	}
	if err != nil {
		return err
	}
	existing.Spec = desired.Spec
	return r.Update(ctx, existing)
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

func (r *EnvironmentReconciler) ensureNamespace(ctx context.Context, name, orgID, projectSlug, envSlug string) error {
	// authd reads enzarb.io/org-id and enzarb.io/project-slug to authorize a
	// deploy pod's pull-only registry scope (see authd deployIdentity). Metering
	// reads enzarb.io/env-slug since the namespace name is no longer reliably
	// parseable (see deployNamespaceName).
	labels := map[string]string{
		"app.kubernetes.io/managed-by": "enzarb-operator",
		"enzarb.io/type":               "deploy",
		"enzarb.io/org-id":             orgID,
		"enzarb.io/project-slug":       projectSlug,
		"enzarb.io/env-slug":           envSlug,
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
