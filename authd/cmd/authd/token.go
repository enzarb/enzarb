package main

import (
	"fmt"
	"strings"
)

// Access is one entry in a Docker registry token's `access` claim.
type Access struct {
	Type    string   `json:"type"`
	Name    string   `json:"name"`
	Actions []string `json:"actions"`
}

// Identity is the authenticated caller. A workspace is identified by its
// org slug + project slug; the app's server-side UI authenticates as Admin.
// Registry and Gitea paths are keyed by the human-readable org slug, so the
// raw org id (UUID) from the namespace is resolved to a slug before we build
// an Identity.
type Identity struct {
	OrgSlug     string
	ProjectSlug string
	Admin       bool
}

// repoPrefix is the only repository path a project identity may access:
// "<orgSlug>/<projectSlug>/". The trailing slash makes prefix checks exact, so
// "<orgSlug>/<projectSlug>" cannot match a sibling like "<orgSlug>/<other>".
func (id Identity) repoPrefix() string {
	return fmt.Sprintf("%s/%s/", id.OrgSlug, id.ProjectSlug)
}

// saRef is the ServiceAccount coordinates parsed from a TokenReview username.
// The org is identified by its id (UUID) here; the caller resolves it to a slug.
type saRef struct {
	OrgID       string
	ProjectSlug string
}

// parseServiceAccountUsername parses the TokenReview username
// "system:serviceaccount:user-<orgId>:<projectSlug>-sa".
func parseServiceAccountUsername(username string) (saRef, error) {
	const saPrefix = "system:serviceaccount:"
	rest, ok := strings.CutPrefix(username, saPrefix)
	if !ok {
		return saRef{}, fmt.Errorf("not a service account: %q", username)
	}
	ns, sa, ok := strings.Cut(rest, ":")
	if !ok {
		return saRef{}, fmt.Errorf("malformed service account: %q", username)
	}
	orgID, ok := strings.CutPrefix(ns, "user-")
	if !ok || orgID == "" {
		return saRef{}, fmt.Errorf("namespace %q is not an org namespace", ns)
	}
	slug, ok := strings.CutSuffix(sa, "-sa")
	if !ok || slug == "" {
		return saRef{}, fmt.Errorf("service account %q is not a project SA", sa)
	}
	return saRef{OrgID: orgID, ProjectSlug: slug}, nil
}

// Scope is a parsed registry scope request, e.g. "repository:<name>:pull,push".
type Scope struct {
	Type    string
	Name    string
	Actions []string
}

// parseScope parses one resourcescope token. Per the Docker token-auth grammar
// the form is `type:name:action[,action]*`; the resource name may contain '/'
// but not ':', so we keep the first and last fields and rejoin any middle.
func parseScope(s string) (Scope, error) {
	parts := strings.Split(s, ":")
	if len(parts) < 3 {
		return Scope{}, fmt.Errorf("malformed scope %q", s)
	}
	name := strings.Join(parts[1:len(parts)-1], ":")
	return Scope{
		Type:    parts[0],
		Name:    name,
		Actions: splitActions(parts[len(parts)-1]),
	}, nil
}

// grant returns the subset of the requested actions this identity is allowed
// for the given scope. An empty result means access is denied.
func (id Identity) grant(sc Scope) []string {
	switch sc.Type {
	case "repository":
		if id.Admin {
			// The app (UI) lists and prunes images for any repo, but never pushes.
			return intersect(sc.Actions, "pull", "delete")
		}
		if strings.HasPrefix(sc.Name, id.repoPrefix()) {
			return intersect(sc.Actions, "pull", "push")
		}
		return nil
	case "registry":
		// Only the admin (UI) may enumerate the catalog.
		if id.Admin && sc.Name == "catalog" {
			return intersect(sc.Actions, "*")
		}
		return nil
	default:
		return nil
	}
}

// grantAll builds the access list for every requested scope, dropping any that
// resolve to no allowed actions.
func (id Identity) grantAll(scopes []string) []Access {
	var out []Access
	for _, raw := range scopes {
		for _, field := range strings.Fields(raw) {
			sc, err := parseScope(field)
			if err != nil {
				continue
			}
			actions := id.grant(sc)
			if len(actions) == 0 {
				continue
			}
			out = append(out, Access{Type: sc.Type, Name: sc.Name, Actions: actions})
		}
	}
	return out
}

func splitActions(s string) []string {
	if s == "" {
		return nil
	}
	return strings.Split(s, ",")
}

// intersect returns the elements of requested that are in allowed, preserving
// the requested order.
func intersect(requested []string, allowed ...string) []string {
	set := make(map[string]bool, len(allowed))
	for _, a := range allowed {
		set[a] = true
	}
	var out []string
	for _, r := range requested {
		if set[r] {
			out = append(out, r)
		}
	}
	return out
}
