package gitea

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"

	"github.com/go-resty/resty/v2"
)

// randomPassword returns a 32-hex-char password for provisioned users. They
// authenticate via reverse-proxy auth, so this is never used to log in.
func randomPassword() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}

type Client struct {
	http    *resty.Client
	baseURL string
}

type Repo struct {
	ID       int    `json:"id"`
	FullName string `json:"full_name"`
	HTMLURL  string `json:"html_url"`
	SSHURL   string `json:"ssh_url"`
}

type CreateRepoRequest struct {
	Name          string `json:"name"`
	Description   string `json:"description,omitempty"`
	Private       bool   `json:"private"`
	AutoInit      bool   `json:"auto_init"`
	DefaultBranch string `json:"default_branch,omitempty"`
}

func NewClient(baseURL, adminToken string) *Client {
	http := resty.New().
		SetBaseURL(baseURL).
		SetHeader("Authorization", fmt.Sprintf("token %s", adminToken)).
		SetHeader("Content-Type", "application/json")
	return &Client{http: http, baseURL: baseURL}
}

// EnsureOrg creates a Gitea organization if it doesn't already exist.
func (c *Client) EnsureOrg(orgSlug string) error {
	resp, err := c.http.R().Get(fmt.Sprintf("/api/v1/orgs/%s", orgSlug))
	if err != nil {
		return err
	}
	if resp.StatusCode() == 200 {
		return nil // already exists
	}
	_, err = c.http.R().
		SetBody(map[string]string{
			"username":   orgSlug,
			"visibility": "private",
		}).
		Post("/api/v1/orgs")
	return err
}

// CreateRepo creates a repository in the given org. Returns existing repo if it already exists.
func (c *Client) CreateRepo(orgSlug string, req CreateRepoRequest) (*Repo, error) {
	var repo Repo

	// Check if already exists
	checkResp, err := c.http.R().
		SetResult(&repo).
		Get(fmt.Sprintf("/api/v1/repos/%s/%s", orgSlug, req.Name))
	if err == nil && checkResp.StatusCode() == 200 {
		return &repo, nil
	}

	resp, err := c.http.R().
		SetBody(req).
		SetResult(&repo).
		Post(fmt.Sprintf("/api/v1/orgs/%s/repos", orgSlug))
	if err != nil {
		return nil, err
	}
	if resp.StatusCode() >= 400 {
		return nil, fmt.Errorf("gitea create repo failed: %s", resp.Body())
	}
	return &repo, nil
}

// EnsureUser creates a Gitea user if absent. The user never logs in with a
// password — the workspace authenticates via reverse-proxy auth (X-Gitea-User,
// set by authd after validating the SA token) — so we set a random unusable
// password. Provisioning explicitly (rather than relying on auto-registration)
// lets us grant repo access deterministically.
func (c *Client) EnsureUser(username, email string) error {
	resp, err := c.http.R().Get(fmt.Sprintf("/api/v1/users/%s", username))
	if err != nil {
		return err
	}
	if resp.StatusCode() == 200 {
		return nil
	}
	createResp, err := c.http.R().
		SetBody(map[string]any{
			"username":             username,
			"email":                email,
			"password":             randomPassword(),
			"must_change_password": false,
		}).
		Post("/api/v1/admin/users")
	if err != nil {
		return err
	}
	if createResp.StatusCode() >= 400 {
		return fmt.Errorf("gitea create user failed: %s", createResp.Body())
	}
	return nil
}

// AddCollaborator grants a user a permission level on a repo ("read"/"write"/
// "admin"). Idempotent — Gitea treats a repeat PUT as an update.
func (c *Client) AddCollaborator(orgSlug, repoName, username, permission string) error {
	resp, err := c.http.R().
		SetBody(map[string]string{"permission": permission}).
		Put(fmt.Sprintf("/api/v1/repos/%s/%s/collaborators/%s", orgSlug, repoName, username))
	if err != nil {
		return err
	}
	if resp.StatusCode() >= 400 {
		return fmt.Errorf("gitea add collaborator failed: %s", resp.Body())
	}
	return nil
}

// RegisterRunnerToken generates a registration token for act_runner.
func (c *Client) RegisterRunnerToken(orgSlug, repo string) (string, error) {
	var result struct {
		Token string `json:"token"`
	}
	resp, err := c.http.R().
		SetResult(&result).
		Post(fmt.Sprintf("/api/v1/repos/%s/%s/actions/runners/registration-token", orgSlug, repo))
	if err != nil {
		return "", err
	}
	if resp.StatusCode() >= 400 {
		return "", fmt.Errorf("gitea runner token failed: %s", resp.Body())
	}
	return result.Token, nil
}

// GetCloneURL returns the HTTPS clone URL for a repo (uses SA token auth).
func (c *Client) GetCloneURL(orgSlug, repoName string) string {
	return fmt.Sprintf("%s/%s/%s.git", c.baseURL, orgSlug, repoName)
}
