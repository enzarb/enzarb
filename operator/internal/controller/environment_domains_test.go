package controller

import (
	"context"
	"errors"
	"net"
	"regexp"
	"testing"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
)

type stubResolver struct {
	records []string
	err     error
}

func (s stubResolver) LookupTXT(_ context.Context, _ string) ([]string, error) {
	return s.records, s.err
}

func withResolver(r interface {
	LookupTXT(ctx context.Context, name string) ([]string, error)
}, fn func()) {
	prev := dnsResolver
	dnsResolver = r
	defer func() { dnsResolver = prev }()
	fn()
}

func TestVerifyDomainTXT(t *testing.T) {
	const token = "ABC123TOKEN"

	tests := []struct {
		name    string
		stub    stubResolver
		want    bool
		wantErr bool
	}{
		{"match", stubResolver{records: []string{challengePrefix + token}}, true, false},
		{"match among many", stubResolver{records: []string{"unrelated", challengePrefix + token}}, true, false},
		{"wrong token", stubResolver{records: []string{challengePrefix + "nope"}}, false, false},
		{"no records", stubResolver{records: nil}, false, false},
		{"nxdomain is not an error", stubResolver{err: &net.DNSError{IsNotFound: true}}, false, false},
		{"temporary is not an error", stubResolver{err: &net.DNSError{IsTemporary: true}}, false, false},
		{"hard error", stubResolver{err: errors.New("boom")}, false, true},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			withResolver(tc.stub, func() {
				got, err := verifyDomainTXT(context.Background(), "app.example.com", token)
				if (err != nil) != tc.wantErr {
					t.Fatalf("err = %v, wantErr %v", err, tc.wantErr)
				}
				if got != tc.want {
					t.Fatalf("got %v, want %v", got, tc.want)
				}
			})
		})
	}
}

func TestClaimNameDeterministicAndScoped(t *testing.T) {
	a := claimName("app.example.com")
	b := claimName("APP.EXAMPLE.COM") // case-insensitive
	if a != b {
		t.Fatalf("claimName not case-insensitive: %q vs %q", a, b)
	}
	if claimName("app.example.com") == claimName("other.example.com") {
		t.Fatal("distinct FQDNs produced the same claim name")
	}
	if len(a) == 0 || a[:3] != "dc-" {
		t.Fatalf("unexpected claim name format: %q", a)
	}
}

func TestGenerateSubdomainIsValidLabel(t *testing.T) {
	re := regexp.MustCompile(`^[a-z][a-z0-9]*$`)
	seen := map[string]bool{}
	for i := 0; i < 50; i++ {
		s, err := generateSubdomain()
		if err != nil {
			t.Fatal(err)
		}
		if !re.MatchString(s) || len(s) > 63 {
			t.Fatalf("invalid DNS label: %q", s)
		}
		if seen[s] {
			t.Fatalf("duplicate subdomain generated: %q", s)
		}
		seen[s] = true
	}
}

func TestServingDomainsUsesDeployZone(t *testing.T) {
	t.Setenv("DEPLOY_DOMAIN", "apps.example.com")
	env := &enzarbv1alpha1.Environment{}
	env.Status.Subdomain = "k7m2x9qf4r"
	got := servingDomains(env)
	if len(got) != 1 || got[0] != "k7m2x9qf4r.apps.example.com" {
		t.Fatalf("unexpected serving domains: %v", got)
	}
}

func TestGenerateTokenUnique(t *testing.T) {
	a, err := generateToken()
	if err != nil {
		t.Fatal(err)
	}
	b, err := generateToken()
	if err != nil {
		t.Fatal(err)
	}
	if a == b || a == "" {
		t.Fatalf("tokens not unique/non-empty: %q %q", a, b)
	}
}
