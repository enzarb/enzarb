package webhook

import (
	"testing"

	corev1 "k8s.io/api/core/v1"
)

func envValue(c corev1.Container, name string) (string, bool) {
	for _, e := range c.Env {
		if e.Name == name {
			return e.Value, true
		}
	}
	return "", false
}

func TestMutatePodInjectsProxyEnv(t *testing.T) {
	pod := &corev1.Pod{
		Spec: corev1.PodSpec{
			InitContainers: []corev1.Container{{Name: "init"}},
			Containers: []corev1.Container{
				{Name: "app", Env: []corev1.EnvVar{{Name: "KUBERNETES_SERVICE_HOST", Value: "10.43.0.1"}}},
			},
		},
	}

	mutatePod(pod)

	for _, c := range append(pod.Spec.InitContainers, pod.Spec.Containers...) {
		if got, ok := envValue(c, "KUBERNETES_SERVICE_HOST"); !ok || got != proxyHost {
			t.Errorf("container %s KUBERNETES_SERVICE_HOST = %q, %v; want %q", c.Name, got, ok, proxyHost)
		}
		if got, ok := envValue(c, "KUBERNETES_SERVICE_PORT"); !ok || got != proxyPort {
			t.Errorf("container %s KUBERNETES_SERVICE_PORT = %q, %v; want %q", c.Name, got, ok, proxyPort)
		}
	}

	// The pre-existing 10.43.0.1 value must be overwritten, not duplicated.
	count := 0
	for _, e := range pod.Spec.Containers[0].Env {
		if e.Name == "KUBERNETES_SERVICE_HOST" {
			count++
		}
	}
	if count != 1 {
		t.Errorf("KUBERNETES_SERVICE_HOST appears %d times; want 1", count)
	}
}

func TestMutatePodSwapsServiceAccountCA(t *testing.T) {
	pod := &corev1.Pod{
		Spec: corev1.PodSpec{
			Containers: []corev1.Container{{Name: "app"}},
			Volumes: []corev1.Volume{
				{
					Name: "kube-api-access-abc12",
					VolumeSource: corev1.VolumeSource{
						Projected: &corev1.ProjectedVolumeSource{
							Sources: []corev1.VolumeProjection{
								{ServiceAccountToken: &corev1.ServiceAccountTokenProjection{Path: "token"}},
								{ConfigMap: &corev1.ConfigMapProjection{
									LocalObjectReference: corev1.LocalObjectReference{Name: clusterRootCAConfigMap},
								}},
								{DownwardAPI: &corev1.DownwardAPIProjection{}},
							},
						},
					},
				},
				// Unrelated ConfigMap volume must be left untouched.
				{
					Name: "app-config",
					VolumeSource: corev1.VolumeSource{
						Projected: &corev1.ProjectedVolumeSource{
							Sources: []corev1.VolumeProjection{
								{ConfigMap: &corev1.ConfigMapProjection{
									LocalObjectReference: corev1.LocalObjectReference{Name: clusterRootCAConfigMap},
								}},
							},
						},
					},
				},
			},
		},
	}

	mutatePod(pod)

	saCA := pod.Spec.Volumes[0].Projected.Sources[1].ConfigMap.Name
	if saCA != ProxyCAConfigMapName {
		t.Errorf("SA token volume ca.crt source = %q; want %q", saCA, ProxyCAConfigMapName)
	}
	unrelated := pod.Spec.Volumes[1].Projected.Sources[0].ConfigMap.Name
	if unrelated != clusterRootCAConfigMap {
		t.Errorf("unrelated ConfigMap volume changed to %q; want %q", unrelated, clusterRootCAConfigMap)
	}
}
