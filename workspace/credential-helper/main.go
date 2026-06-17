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
	tokenPath    = "/var/run/secrets/enzarb/registry/token"
	registryHost = "registry.enzarb.dev"
)

type Credentials struct {
	ServerURL string `json:"ServerURL"`
	Username  string `json:"Username"`
	Secret    string `json:"Secret"`
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
		json.NewEncoder(os.Stdout).Encode(map[string]string{ //nolint:errcheck
			registryHost: "sa-token",
		})
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
	fmt.Fscan(os.Stdin, &serverURL)
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
	json.NewEncoder(os.Stdout).Encode(creds) //nolint:errcheck
}
