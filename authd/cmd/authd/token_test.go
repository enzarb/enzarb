package main

import (
	"reflect"
	"testing"
)

func TestParseServiceAccountUsername(t *testing.T) {
	tests := []struct {
		username string
		want     saRef
		wantErr  bool
	}{
		{"system:serviceaccount:user-org123:blog-sa", saRef{OrgID: "org123", ProjectSlug: "blog"}, false},
		{"system:serviceaccount:user-org123:my-app-sa", saRef{OrgID: "org123", ProjectSlug: "my-app"}, false},
		{"system:serviceaccount:kube-system:default", saRef{}, true}, // not an org namespace
		{"system:serviceaccount:user-org123:default", saRef{}, true}, // not a project SA
		{"system:serviceaccount:user-:blog-sa", saRef{}, true},       // empty org
		{"alice@example.com", saRef{}, true},                         // not a SA
	}
	for _, tt := range tests {
		got, err := parseServiceAccountUsername(tt.username)
		if (err != nil) != tt.wantErr {
			t.Errorf("%q: err=%v wantErr=%v", tt.username, err, tt.wantErr)
			continue
		}
		if err == nil && got != tt.want {
			t.Errorf("%q: got %+v want %+v", tt.username, got, tt.want)
		}
	}
}

func TestGrantScoping(t *testing.T) {
	proj := Identity{OrgSlug: "orgA", ProjectSlug: "blog"}
	admin := Identity{Admin: true}

	tests := []struct {
		name  string
		id    Identity
		scope string
		want  []string
	}{
		{"own repo push/pull", proj, "repository:orgA/blog/img:pull,push", []string{"pull", "push"}},
		{"own repo nested path", proj, "repository:orgA/blog/sub/img:push", []string{"push"}},
		{"cross-org denied", proj, "repository:orgB/blog/img:pull", nil},
		{"sibling project denied", proj, "repository:orgA/other/img:pull,push", nil},
		{"prefix-spoof denied", proj, "repository:orgA/blog-evil/img:pull", nil},
		{"exact name no slash denied", proj, "repository:orgA/blog:pull", nil},
		{"catalog denied for project", proj, "registry:catalog:*", nil},
		{"admin pull anything", admin, "repository:orgB/x/img:pull,push", []string{"pull"}},
		{"admin catalog", admin, "registry:catalog:*", []string{"*"}},
	}
	for _, tt := range tests {
		sc, err := parseScope(tt.scope)
		if err != nil {
			t.Fatalf("%s: parseScope: %v", tt.name, err)
		}
		got := tt.id.grant(sc)
		if !reflect.DeepEqual(got, tt.want) {
			t.Errorf("%s: got %v want %v", tt.name, got, tt.want)
		}
	}
}

func TestParseScopeMalformed(t *testing.T) {
	for _, s := range []string{"", "repository", "repository:name"} {
		if _, err := parseScope(s); err == nil {
			t.Errorf("parseScope(%q) expected error", s)
		}
	}
}

func TestGrantAllSkipsDenied(t *testing.T) {
	proj := Identity{OrgSlug: "orgA", ProjectSlug: "blog"}
	got := proj.grantAll([]string{"repository:orgA/blog/img:pull,push repository:orgB/x:pull"})
	want := []Access{{Type: "repository", Name: "orgA/blog/img", Actions: []string{"pull", "push"}}}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %+v want %+v", got, want)
	}
}
