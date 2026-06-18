package gitea

import (
	"fmt"

	"github.com/go-resty/resty/v2"
)

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
			"username":    orgSlug,
			"visibility":  "private",
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
