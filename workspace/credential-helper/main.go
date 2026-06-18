// docker-credential-k8s-sa implements the Docker credential helper protocol,
// reading a projected K8s service account token and returning it as a Bearer
// token for registry.enzarb.dev (Zot accepts SA tokens via K8s TokenReview).
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

const (
	tokenPath    = "/var/run/secrets/enzarb/registry/token" //nolint:gosec // not a credential, this is a file path
	registryHost = "registry.enzarb.dev"
)

type Credentials struct {
	ServerURL string `json:"ServerURL"`
	Username  string `json:"Username"`
	Secret    string `json:"Secret"` //nolint:gosec // G117: Docker credential helper protocol requires this field name
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: docker-credential-k8s-sa <get|list|store|erase>")
		os.Exit(1)
	}

	switch os.Args[1] {
	case "get":
		handleGet()
	case "list":
		// Return our registry so Docker knows we handle it
		if err := json.NewEncoder(os.Stdout).Encode(map[string]string{
			registryHost: "sa-token",
		}); err != nil {
			fmt.Fprintf(os.Stderr, "encode list: %v\n", err)
			os.Exit(1)
		}
	case "store", "erase":
		// No-op: tokens come from the projected volume, not from docker login
	default:
		fmt.Fprintf(os.Stderr, "unknown command: %s\n", os.Args[1])
		os.Exit(1)
	}
}

func handleGet() {
	// Read server URL from stdin
	var serverURL string
	if _, err := fmt.Fscan(os.Stdin, &serverURL); err != nil {
		fmt.Fprintf(os.Stderr, "read stdin: %v\n", err)
		os.Exit(1)
	}
	serverURL = strings.TrimSpace(serverURL)

	if !strings.Contains(serverURL, registryHost) {
		fmt.Fprintln(os.Stderr, "not our registry")
		os.Exit(1)
	}

	token, err := os.ReadFile(tokenPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "read SA token: %v\n", err)
		os.Exit(1)
	}

	creds := Credentials{
		ServerURL: serverURL,
		Username:  "sa-token",
		Secret:    strings.TrimSpace(string(token)),
	}
	if err := json.NewEncoder(os.Stdout).Encode(creds); err != nil { //nolint:gosec // G117: Docker credential helper protocol requires "Secret" field name
		fmt.Fprintf(os.Stderr, "encode creds: %v\n", err)
		os.Exit(1)
	}
}
