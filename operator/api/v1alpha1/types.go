package v1alpha1

import (
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"sigs.k8s.io/controller-runtime/pkg/scheme"
)

var (
	GroupVersion  = schema.GroupVersion{Group: "enzarb.io", Version: "v1alpha1"}
	SchemeBuilder = &scheme.Builder{GroupVersion: GroupVersion} //nolint:staticcheck // deprecated but still functional; kubebuilder-generated code uses it
	AddToScheme   = SchemeBuilder.AddToScheme
)

func init() {
	SchemeBuilder.Register(&Project{}, &ProjectList{})
	SchemeBuilder.Register(&Environment{}, &EnvironmentList{})
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Namespaced,shortName=proj
// +kubebuilder:printcolumn:name="Phase",type=string,JSONPath=`.status.phase`
// +kubebuilder:printcolumn:name="Age",type=date,JSONPath=`.metadata.creationTimestamp`

type Project struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   ProjectSpec   `json:"spec,omitempty"`
	Status ProjectStatus `json:"status,omitempty"`
}

type ProjectSpec struct {
	OrgID       string          `json:"orgId"`
	Slug        string          `json:"slug"`
	DisplayName string          `json:"displayName"`
	Tools       []ProjectTool   `json:"tools,omitempty"`
	Storage     ProjectStorage  `json:"storage,omitempty"`
	Resources   corev1.ResourceRequirements `json:"resources,omitempty"`
}

type ProjectTool struct {
	Name    string `json:"name"`
	Version string `json:"version,omitempty"`
}

type ProjectStorage struct {
	Size resource.Quantity `json:"size"`
}

type ProjectStatus struct {
	Phase              string             `json:"phase,omitempty"`
	WorkspacePodName   string             `json:"workspacePodName,omitempty"`
	ServiceAccountName string             `json:"serviceAccountName,omitempty"`
	AgentPath          string             `json:"agentPath,omitempty"`
	Conditions         []metav1.Condition `json:"conditions,omitempty"`
}

// +kubebuilder:object:root=true

type ProjectList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Project `json:"items"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Namespaced,shortName=env
// +kubebuilder:printcolumn:name="Namespace",type=string,JSONPath=`.status.namespace`
// +kubebuilder:printcolumn:name="Age",type=date,JSONPath=`.metadata.creationTimestamp`

type Environment struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   EnvironmentSpec   `json:"spec,omitempty"`
	Status EnvironmentStatus `json:"status,omitempty"`
}

type EnvironmentSpec struct {
	ProjectRef    corev1.LocalObjectReference `json:"projectRef"`
	Slug          string                      `json:"slug"`
	CustomDomains []CustomDomain              `json:"customDomains,omitempty"`
	GatewayRef    GatewayRef                  `json:"gatewayRef,omitempty"`
}

type CustomDomain struct {
	FQDN    string `json:"fqdn"`
	TLSMode string `json:"tlsMode,omitempty"` // acme | byoc
}

type GatewayRef struct {
	Name      string `json:"name"`
	Namespace string `json:"namespace"`
}

type EnvironmentStatus struct {
	Namespace   string             `json:"namespace,omitempty"`
	Domains     []DomainStatus     `json:"domains,omitempty"`
	Conditions  []metav1.Condition `json:"conditions,omitempty"`
}

type DomainStatus struct {
	FQDN       string `json:"fqdn"`
	CertStatus string `json:"certStatus,omitempty"`
	VerifiedAt string `json:"verifiedAt,omitempty"`
}

// +kubebuilder:object:root=true

type EnvironmentList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Environment `json:"items"`
}

// DeepCopyObject implementations (required by runtime.Object)

func (p *Project) DeepCopyObject() runtime.Object {
	if p == nil {
		return nil
	}
	out := new(Project)
	p.DeepCopyInto(out)
	return out
}

func (p *Project) DeepCopyInto(out *Project) {
	*out = *p
	out.TypeMeta = p.TypeMeta
	p.ObjectMeta.DeepCopyInto(&out.ObjectMeta)
	p.Spec.DeepCopyInto(&out.Spec)
	p.Status.DeepCopyInto(&out.Status)
}

func (pl *ProjectList) DeepCopyObject() runtime.Object {
	if pl == nil {
		return nil
	}
	out := new(ProjectList)
	pl.DeepCopyInto(out)
	return out
}

func (pl *ProjectList) DeepCopyInto(out *ProjectList) {
	*out = *pl
	out.TypeMeta = pl.TypeMeta
	pl.ListMeta.DeepCopyInto(&out.ListMeta)
	if pl.Items != nil {
		out.Items = make([]Project, len(pl.Items))
		for i := range pl.Items {
			pl.Items[i].DeepCopyInto(&out.Items[i])
		}
	}
}

func (e *Environment) DeepCopyObject() runtime.Object {
	if e == nil {
		return nil
	}
	out := new(Environment)
	e.DeepCopyInto(out)
	return out
}

func (e *Environment) DeepCopyInto(out *Environment) {
	*out = *e
	out.TypeMeta = e.TypeMeta
	e.ObjectMeta.DeepCopyInto(&out.ObjectMeta)
	e.Spec.DeepCopyInto(&out.Spec)
	e.Status.DeepCopyInto(&out.Status)
}

func (el *EnvironmentList) DeepCopyObject() runtime.Object {
	if el == nil {
		return nil
	}
	out := new(EnvironmentList)
	el.DeepCopyInto(out)
	return out
}

func (el *EnvironmentList) DeepCopyInto(out *EnvironmentList) {
	*out = *el
	out.TypeMeta = el.TypeMeta
	el.ListMeta.DeepCopyInto(&out.ListMeta)
	if el.Items != nil {
		out.Items = make([]Environment, len(el.Items))
		for i := range el.Items {
			el.Items[i].DeepCopyInto(&out.Items[i])
		}
	}
}

func (s *ProjectSpec) DeepCopyInto(out *ProjectSpec) {
	*out = *s
	if s.Tools != nil {
		out.Tools = make([]ProjectTool, len(s.Tools))
		copy(out.Tools, s.Tools)
	}
	out.Storage = ProjectStorage{Size: s.Storage.Size.DeepCopy()}
	s.Resources.DeepCopyInto(&out.Resources)
}

func (s *ProjectStatus) DeepCopyInto(out *ProjectStatus) {
	*out = *s
	if s.Conditions != nil {
		out.Conditions = make([]metav1.Condition, len(s.Conditions))
		copy(out.Conditions, s.Conditions)
	}
}

func (s *EnvironmentSpec) DeepCopyInto(out *EnvironmentSpec) {
	*out = *s
	if s.CustomDomains != nil {
		out.CustomDomains = make([]CustomDomain, len(s.CustomDomains))
		copy(out.CustomDomains, s.CustomDomains)
	}
}

func (s *EnvironmentStatus) DeepCopyInto(out *EnvironmentStatus) {
	*out = *s
	if s.Domains != nil {
		out.Domains = make([]DomainStatus, len(s.Domains))
		copy(out.Domains, s.Domains)
	}
	if s.Conditions != nil {
		out.Conditions = make([]metav1.Condition, len(s.Conditions))
		copy(out.Conditions, s.Conditions)
	}
}
