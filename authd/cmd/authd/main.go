package main

import (
	"context"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/subtle"
	"crypto/x509"
	"encoding/base32"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/golang-jwt/jwt/v5"
	authnv1 "k8s.io/api/authentication/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/dynamic"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"
)

const (
	registryAudience = "registry.enzarb.dev"
	giteaAudience    = "gitea.enzarb.dev"
	adminUsername    = "admin"
	tokenTTL         = 5 * time.Minute
)

// validator authenticates a presented bearer token (a projected K8s SA token)
// for a given audience, returning the workspace Identity.
type validator interface {
	validate(ctx context.Context, token, audience string) (Identity, error)
}

type server struct {
	val       validator
	signKey   *rsa.PrivateKey
	keyID     string
	issuer    string
	adminPass string
}

func main() {
	slog.Info("authd starting")

	keyPath := envOr("TOKEN_SIGNING_KEY", "/etc/authd/signing/tls.key")
	signKey, keyID, err := loadSigningKey(keyPath)
	if err != nil {
		slog.Error("load signing key", "err", err)
		os.Exit(1)
	}

	cfg, err := rest.InClusterConfig()
	if err != nil {
		slog.Error("k8s config", "err", err)
		os.Exit(1)
	}
	clientset, err := kubernetes.NewForConfig(cfg)
	if err != nil {
		slog.Error("k8s client", "err", err)
		os.Exit(1)
	}
	dyn, err := dynamic.NewForConfig(cfg)
	if err != nil {
		slog.Error("k8s dynamic client", "err", err)
		os.Exit(1)
	}

	srv := &server{
		val:       newK8sValidator(clientset, dyn),
		signKey:   signKey,
		keyID:     keyID,
		issuer:    envOr("TOKEN_ISSUER", registryAudience),
		adminPass: os.Getenv("ADMIN_SECRET"),
	}

	mux := http.NewServeMux()
	mux.HandleFunc("GET /auth/token", srv.handleRegistryToken)
	// Envoy Gateway's extAuth prepends this path with the client's original
	// request path and method, so match the whole subtree on any method (git
	// uses GET /info/refs then POST /git-upload-pack). Identity comes from the
	// Authorization header, not the path.
	mux.HandleFunc("/authz/git", srv.handleGitAuthz)
	mux.HandleFunc("/authz/git/", srv.handleGitAuthz)
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) })

	addr := envOr("LISTEN_ADDR", ":8080")
	slog.Info("listening", "addr", addr)
	if err := http.ListenAndServe(addr, mux); err != nil { //nolint:gosec // internal service; fronted by gateway TLS
		slog.Error("serve", "err", err)
		os.Exit(1)
	}
}

// authenticate resolves the caller from HTTP basic auth. The password is either
// the admin shared secret or a projected SA token for the given audience.
func (s *server) authenticate(ctx context.Context, r *http.Request, audience string) (Identity, error) {
	user, pass, ok := r.BasicAuth()
	if !ok || pass == "" {
		return Identity{}, errors.New("missing credentials")
	}
	if user == adminUsername {
		if s.adminPass == "" || subtle.ConstantTimeCompare([]byte(pass), []byte(s.adminPass)) != 1 {
			return Identity{}, errors.New("invalid admin credentials")
		}
		return Identity{Admin: true}, nil
	}
	return s.val.validate(ctx, pass, audience)
}

// handleRegistryToken implements the Docker Registry v2 token endpoint. Zot
// redirects unauthenticated clients here; we mint a JWT scoped to exactly what
// the caller is allowed to access.
func (s *server) handleRegistryToken(w http.ResponseWriter, r *http.Request) {
	id, err := s.authenticate(r.Context(), r, registryAudience)
	if err != nil {
		// No credentials at all → 401 so the client retries with auth. Bad
		// credentials → also 401 per the token-auth flow.
		w.Header().Set("WWW-Authenticate", fmt.Sprintf(`Bearer realm="https://%s/auth/token",service="%s"`, registryAudience, registryAudience))
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}

	access := id.grantAll(r.URL.Query()["scope"])
	tok, err := s.mintRegistryToken(id, r.URL.Query().Get("service"), access)
	if err != nil {
		slog.Error("mint token", "err", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	// Both keys for client compatibility (older Docker reads `token`).
	if err := json.NewEncoder(w).Encode(map[string]any{
		"token":        tok,
		"access_token": tok,
		"expires_in":   int(tokenTTL.Seconds()),
	}); err != nil {
		slog.Error("write token response", "err", err)
	}
}

// handleGitAuthz is the Envoy Gateway extAuth check fronting Gitea. On success
// it returns the resolved identity as X-Gitea-User, which Gitea trusts via
// reverse-proxy authentication.
func (s *server) handleGitAuthz(w http.ResponseWriter, r *http.Request) {
	id, err := s.authenticate(r.Context(), r, giteaAudience)
	if err != nil || id.Admin {
		w.Header().Set("WWW-Authenticate", `Basic realm="enzarb"`)
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	// Deterministic, Gitea-safe username tying the workspace to its org/project.
	w.Header().Set("X-Gitea-User", fmt.Sprintf("%s--%s", id.OrgSlug, id.ProjectSlug))
	w.WriteHeader(http.StatusOK)
}

type registryClaims struct {
	jwt.RegisteredClaims
	Access []Access `json:"access"`
}

func (s *server) mintRegistryToken(id Identity, service string, access []Access) (string, error) {
	if service == "" {
		service = registryAudience
	}
	now := time.Now()
	subject := "admin"
	if !id.Admin {
		subject = fmt.Sprintf("%s/%s", id.OrgSlug, id.ProjectSlug)
	}
	claims := registryClaims{
		RegisteredClaims: jwt.RegisteredClaims{
			Issuer:    s.issuer,
			Subject:   subject,
			Audience:  jwt.ClaimStrings{service},
			IssuedAt:  jwt.NewNumericDate(now),
			NotBefore: jwt.NewNumericDate(now.Add(-30 * time.Second)),
			ExpiresAt: jwt.NewNumericDate(now.Add(tokenTTL)),
			ID:        fmt.Sprintf("%d", now.UnixNano()),
		},
		Access: access,
	}
	tok := jwt.NewWithClaims(jwt.SigningMethodRS256, claims)
	// Zot matches the JWT's `kid` against the libtrust key id of its trusted cert.
	tok.Header["kid"] = s.keyID
	return tok.SignedString(s.signKey)
}

// loadSigningKey reads a PEM RSA private key and derives the libtrust key id of
// its public key (the format Zot/distribution use to match `kid`).
func loadSigningKey(path string) (*rsa.PrivateKey, string, error) {
	pemBytes, err := os.ReadFile(path) //nolint:gosec // path is operator-controlled config
	if err != nil {
		return nil, "", fmt.Errorf("read key: %w", err)
	}
	block, _ := pem.Decode(pemBytes)
	if block == nil {
		return nil, "", errors.New("no PEM block in signing key")
	}
	key, err := parseRSAPrivateKey(block.Bytes)
	if err != nil {
		return nil, "", err
	}
	kid, err := libtrustKeyID(&key.PublicKey)
	if err != nil {
		return nil, "", err
	}
	return key, kid, nil
}

func parseRSAPrivateKey(der []byte) (*rsa.PrivateKey, error) {
	if k, err := x509.ParsePKCS1PrivateKey(der); err == nil {
		return k, nil
	}
	k, err := x509.ParsePKCS8PrivateKey(der)
	if err != nil {
		return nil, fmt.Errorf("parse private key: %w", err)
	}
	rsaKey, ok := k.(*rsa.PrivateKey)
	if !ok {
		return nil, fmt.Errorf("signing key is %T, want RSA", k)
	}
	return rsaKey, nil
}

// libtrustKeyID computes the key id as base32(SHA256(DER public key)[:30]),
// grouped into 12 colon-separated quads — the docker/libtrust convention that
// distribution-based registries (incl. Zot) expect in the token `kid` header.
func libtrustKeyID(pub *rsa.PublicKey) (string, error) {
	der, err := x509.MarshalPKIXPublicKey(pub)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256(der)
	enc := base32.StdEncoding.EncodeToString(sum[:30])
	var quads []string
	for i := 0; i < len(enc); i += 4 {
		quads = append(quads, enc[i:i+4])
	}
	return strings.Join(quads, ":"), nil
}

// organizationsGVR is the cluster-scoped Organization CR (name == org id).
var organizationsGVR = schema.GroupVersionResource{
	Group:    "enzarb.io",
	Version:  "v1alpha1",
	Resource: "organizations",
}

const slugCacheTTL = 5 * time.Minute

// k8sValidator authenticates SA tokens via the TokenReview API and resolves the
// org id (from the SA namespace) to the human-readable org slug used in registry
// and Gitea paths.
type k8sValidator struct {
	client kubernetes.Interface
	dyn    dynamic.Interface

	mu    sync.RWMutex
	cache map[string]slugEntry
}

type slugEntry struct {
	slug    string
	expires time.Time
}

func newK8sValidator(client kubernetes.Interface, dyn dynamic.Interface) *k8sValidator {
	return &k8sValidator{client: client, dyn: dyn, cache: map[string]slugEntry{}}
}

func (v *k8sValidator) validate(ctx context.Context, token, audience string) (Identity, error) {
	review, err := v.client.AuthenticationV1().TokenReviews().Create(ctx, &authnv1.TokenReview{
		Spec: authnv1.TokenReviewSpec{
			Token:     token,
			Audiences: []string{audience},
		},
	}, metav1.CreateOptions{})
	if err != nil {
		return Identity{}, fmt.Errorf("token review: %w", err)
	}
	if !review.Status.Authenticated {
		return Identity{}, fmt.Errorf("token not authenticated: %s", review.Status.Error)
	}
	// Ensure the token was actually issued for this audience.
	if !contains(review.Status.Audiences, audience) {
		return Identity{}, fmt.Errorf("token audience mismatch")
	}
	ref, err := parseServiceAccountUsername(review.Status.User.Username)
	if err != nil {
		return Identity{}, err
	}
	slug, err := v.resolveSlug(ctx, ref.OrgID)
	if err != nil {
		return Identity{}, err
	}
	return Identity{OrgSlug: slug, ProjectSlug: ref.ProjectSlug}, nil
}

// resolveSlug maps an org id (UUID) to its slug via the Organization CR, cached
// briefly so a slug change is picked up without restart but TokenReviews aren't
// followed by a CR GET on every request.
func (v *k8sValidator) resolveSlug(ctx context.Context, orgID string) (string, error) {
	v.mu.RLock()
	if e, ok := v.cache[orgID]; ok && time.Now().Before(e.expires) {
		v.mu.RUnlock()
		return e.slug, nil
	}
	v.mu.RUnlock()

	obj, err := v.dyn.Resource(organizationsGVR).Get(ctx, orgID, metav1.GetOptions{})
	if err != nil {
		return "", fmt.Errorf("get organization %q: %w", orgID, err)
	}
	slug, found, err := unstructuredString(obj.Object, "spec", "slug")
	if err != nil || !found || slug == "" {
		return "", fmt.Errorf("organization %q has no spec.slug", orgID)
	}

	v.mu.Lock()
	v.cache[orgID] = slugEntry{slug: slug, expires: time.Now().Add(slugCacheTTL)}
	v.mu.Unlock()
	return slug, nil
}

// unstructuredString reads a nested string field from an unstructured object.
func unstructuredString(obj map[string]any, fields ...string) (string, bool, error) {
	cur := any(obj)
	for _, f := range fields {
		m, ok := cur.(map[string]any)
		if !ok {
			return "", false, nil
		}
		cur, ok = m[f]
		if !ok {
			return "", false, nil
		}
	}
	s, ok := cur.(string)
	if !ok {
		return "", false, fmt.Errorf("field %v is not a string", fields)
	}
	return s, true, nil
}

func contains(haystack []string, needle string) bool {
	for _, h := range haystack {
		if h == needle {
			return true
		}
	}
	return false
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
