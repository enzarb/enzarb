// Package webhook contains the operator's admission webhooks.
package webhook

import (
	"context"
	"encoding/json"
	"net/http"

	corev1 "k8s.io/api/core/v1"
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
)

// Deploy-namespace pods must never reach the Kubernetes API server directly:
// tenant isolation depends on all cluster-scoped list/watch traffic being
// filtered by capsule-proxy (see internal/controller/capsule.go). The
// enzarb-deploy-isolation NetworkPolicy already blocks the service CIDR
// (including kubernetes.default at 10.43.0.1), so a pod using the default
// in-cluster config just times out — which is exactly what K3s helm-controller's
// helm-install Job hit.
//
// This webhook rewrites every pod created in a deploy namespace to talk to
// capsule-proxy instead. Two mutations are required and neither works without
// the other:
//
//  1. Point client-go's in-cluster config at the proxy by overriding the
//     KUBERNETES_SERVICE_HOST/PORT env vars the kubelet injects.
//  2. Make the pod trust the proxy's serving cert. client-go reads the CA from
//     a fixed path inside the ServiceAccount token volume, which the in-tree
//     ServiceAccount admission plugin has already populated with the API
//     server CA (kube-root-ca.crt) by the time this webhook runs. We swap that
//     ConfigMap reference for the per-namespace capsule-proxy CA ConfigMap the
//     operator maintains (ensureProxyCAConfigMap), so ca.crt becomes the proxy
//     CA without disturbing the projected token or downwardAPI sources.
const (
	// ProxyHost/ProxyPort are capsule-proxy's in-cluster address (deploy/system/capsule.yaml).
	proxyHost = "capsule-proxy.capsule-system.svc"
	proxyPort = "9001"

	// ProxyCAConfigMapName is the per-deploy-namespace ConfigMap (key ca.crt)
	// holding the capsule-proxy CA; see EnvironmentReconciler.ensureProxyCAConfigMap.
	ProxyCAConfigMapName = "capsule-proxy-ca"

	// clusterRootCAConfigMap is the ConfigMap the ServiceAccount admission
	// plugin references for the API server CA in the projected token volume.
	clusterRootCAConfigMap = "kube-root-ca.crt"
)

// PodProxyInjector is the /mutate-v1-pod-proxy admission handler.
type PodProxyInjector struct {
	decoder admission.Decoder
}

// NewPodProxyInjector builds the handler with a decoder for the given scheme.
func NewPodProxyInjector(decoder admission.Decoder) *PodProxyInjector {
	return &PodProxyInjector{decoder: decoder}
}

// Handle rewrites the pod's API access to route through capsule-proxy.
func (p *PodProxyInjector) Handle(_ context.Context, req admission.Request) admission.Response {
	pod := &corev1.Pod{}
	if err := p.decoder.Decode(req, pod); err != nil {
		return admission.Errored(http.StatusBadRequest, err)
	}

	mutatePod(pod)

	marshaled, err := json.Marshal(pod)
	if err != nil {
		return admission.Errored(http.StatusInternalServerError, err)
	}
	return admission.PatchResponseFromRaw(req.Object.Raw, marshaled)
}

// mutatePod applies both the endpoint override and the CA swap in place.
func mutatePod(pod *corev1.Pod) {
	for i := range pod.Spec.InitContainers {
		setProxyEnv(&pod.Spec.InitContainers[i])
	}
	for i := range pod.Spec.Containers {
		setProxyEnv(&pod.Spec.Containers[i])
	}
	swapServiceAccountCA(pod)
}

// setProxyEnv forces the in-cluster API endpoint env vars, overriding whatever
// the kubelet would otherwise inject (KUBERNETES_SERVICE_HOST/PORT) or any the
// image sets, so client-go dials capsule-proxy.
func setProxyEnv(c *corev1.Container) {
	setEnv(c, "KUBERNETES_SERVICE_HOST", proxyHost)
	setEnv(c, "KUBERNETES_SERVICE_PORT", proxyPort)
	setEnv(c, "KUBERNETES_SERVICE_PORT_HTTPS", proxyPort)
}

func setEnv(c *corev1.Container, name, value string) {
	for i := range c.Env {
		if c.Env[i].Name == name {
			c.Env[i].Value = value
			c.Env[i].ValueFrom = nil
			return
		}
	}
	c.Env = append(c.Env, corev1.EnvVar{Name: name, Value: value})
}

// swapServiceAccountCA repoints the CA source of the projected ServiceAccount
// token volume(s) from the API server CA (kube-root-ca.crt) to the capsule-proxy
// CA ConfigMap. It only touches projected volumes that carry a ServiceAccount
// token source, leaving unrelated ConfigMap volumes alone. The token and
// downwardAPI namespace sources are untouched, so the SA token capsule-proxy
// authenticates with (via TokenReview) is unchanged.
func swapServiceAccountCA(pod *corev1.Pod) {
	for i := range pod.Spec.Volumes {
		proj := pod.Spec.Volumes[i].Projected
		if proj == nil {
			continue
		}
		hasToken := false
		for j := range proj.Sources {
			if proj.Sources[j].ServiceAccountToken != nil {
				hasToken = true
				break
			}
		}
		if !hasToken {
			continue
		}
		for j := range proj.Sources {
			cm := proj.Sources[j].ConfigMap
			if cm != nil && cm.Name == clusterRootCAConfigMap {
				cm.Name = ProxyCAConfigMapName
			}
		}
	}
}
