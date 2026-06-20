package main

import (
	"crypto/rand"
	"crypto/rsa"
	"regexp"
	"testing"

	"github.com/golang-jwt/jwt/v5"
)

func TestMintRegistryToken(t *testing.T) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	kid, err := libtrustKeyID(&key.PublicKey)
	if err != nil {
		t.Fatal(err)
	}
	// 12 colon-separated 4-char base32 quads.
	if !regexp.MustCompile(`^([A-Z2-7]{4}:){11}[A-Z2-7]{4}$`).MatchString(kid) {
		t.Fatalf("kid %q not in libtrust quad format", kid)
	}

	s := &server{signKey: key, keyID: kid, issuer: registryAudience}
	id := Identity{OrgSlug: "orgA", ProjectSlug: "blog"}
	access := id.grantAll([]string{"repository:orgA/blog/img:pull,push"})

	raw, err := s.mintRegistryToken(id, "registry.enzarb.dev", access)
	if err != nil {
		t.Fatal(err)
	}

	var claims registryClaims
	tok, err := jwt.ParseWithClaims(raw, &claims, func(tok *jwt.Token) (any, error) {
		if tok.Header["kid"] != kid {
			t.Errorf("kid header = %v, want %v", tok.Header["kid"], kid)
		}
		return &key.PublicKey, nil
	})
	if err != nil || !tok.Valid {
		t.Fatalf("verify: err=%v valid=%v", err, tok.Valid)
	}
	if len(claims.Access) != 1 || claims.Access[0].Name != "orgA/blog/img" {
		t.Fatalf("unexpected access claim: %+v", claims.Access)
	}
	if claims.Subject != "orgA/blog" {
		t.Errorf("subject = %q, want orgA/blog", claims.Subject)
	}
}
