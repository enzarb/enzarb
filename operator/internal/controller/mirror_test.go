package controller

import (
	"strings"
	"testing"
)

func TestRenderBuildkitToml(t *testing.T) {
	host := "enzarb-mirror.enzarb-system.svc.cluster.local:5000"
	toml := renderBuildkitToml(host)

	for upstream, prefix := range mirrorUpstreams {
		entry := `[registry."` + upstream + `"]`
		if !strings.Contains(toml, entry) {
			t.Errorf("missing %s in:\n%s", entry, toml)
		}
		mirror := `mirrors = ["` + host + `/` + prefix + `"]`
		if !strings.Contains(toml, mirror) {
			t.Errorf("missing %s in:\n%s", mirror, toml)
		}
	}
	if !strings.Contains(toml, `[registry."`+host+`"]`+"\n  http = true") {
		t.Errorf("missing plaintext marker for mirror host in:\n%s", toml)
	}
}

func TestBuildkitSidecarConfigGatedOnMirror(t *testing.T) {
	t.Setenv("MIRROR_ENABLED", "true")
	t.Setenv("MIRROR_HOST", "enzarb-mirror.enzarb-system.svc.cluster.local:5000")

	args := buildkitArgs()
	if !contains(args, "--config") {
		t.Errorf("expected --config in args when mirror enabled: %v", args)
	}
	if mounts := buildkitVolumeMounts(); len(mounts) != 1 || mounts[0].MountPath != "/etc/buildkit" {
		t.Errorf("expected /etc/buildkit mount when mirror enabled: %v", mounts)
	}
	if vols := buildkitConfigVolumeSlice(); len(vols) != 1 || vols[0].ConfigMap.Name != buildkitConfigMapName {
		t.Errorf("expected %s ConfigMap volume when mirror enabled: %v", buildkitConfigMapName, vols)
	}

	t.Setenv("MIRROR_ENABLED", "false")
	if args := buildkitArgs(); contains(args, "--config") {
		t.Errorf("unexpected --config in args when mirror disabled: %v", args)
	}
	if mounts := buildkitVolumeMounts(); len(mounts) != 0 {
		t.Errorf("unexpected mounts when mirror disabled: %v", mounts)
	}
	if vols := buildkitConfigVolumeSlice(); len(vols) != 0 {
		t.Errorf("unexpected volumes when mirror disabled: %v", vols)
	}

	// Enabled but no host configured: treated as disabled.
	t.Setenv("MIRROR_ENABLED", "true")
	t.Setenv("MIRROR_HOST", "")
	if _, enabled := mirrorEnabled(); enabled {
		t.Error("mirror should be disabled without MIRROR_HOST")
	}
}

func contains(s []string, v string) bool {
	for _, x := range s {
		if x == v {
			return true
		}
	}
	return false
}
