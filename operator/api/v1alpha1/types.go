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
	SchemeBuilder.Register(&Organization{}, &OrganizationList{})
	SchemeBuilder.Register(&AllowedDomains{}, &AllowedDomainsList{})
	SchemeBuilder.Register(&DomainClaim{}, &DomainClaimList{})
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Cluster,shortName=org
// +kubebuilder:printcolumn:name="Slug",type=string,JSONPath=`.spec.slug`
// +kubebuilder:printcolumn:name="Phase",type=string,JSONPath=`.status.phase`
// +kubebuilder:printcolumn:name="Age",type=date,JSONPath=`.metadata.creationTimestamp`

// Organization is a cluster-scoped resource that owns a tenant's namespace. The
// app creates one per org at org-creation time so the operator — not the app —
// provisions and owns the `user-<orgId>` namespace before any namespace-scoped
// Project is created inside it.
type Organization struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   OrganizationSpec   `json:"spec,omitempty"`
	Status OrganizationStatus `json:"status,omitempty"`
}

type OrganizationSpec struct {
	OrgID       string `json:"orgId"`
	Slug        string `json:"slug"`
	DisplayName string `json:"displayName,omitempty"`
}

type OrganizationStatus struct {
	Phase      string             `json:"phase,omitempty"`
	Namespace  string             `json:"namespace,omitempty"`
	Conditions []metav1.Condition `json:"conditions,omitempty"`
}

// +kubebuilder:object:root=true

type OrganizationList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Organization `json:"items"`
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
	OrgID       string                      `json:"orgId"`
	Slug        string                      `json:"slug"`
	DisplayName string                      `json:"displayName"`
	Tools       []ProjectTool               `json:"tools,omitempty"`
	Storage     ProjectStorage              `json:"storage,omitempty"`
	Resources   corev1.ResourceRequirements `json:"resources,omitempty"`
	// GPUEnabled is an admin-only flag. When true the workspace Pod requests
	// one nvidia.com/gpu and is scheduled onto a GPU-tainted node.
	GPUEnabled bool `json:"gpuEnabled,omitempty"`
	// Suspended is a reversible, user-initiated shutdown: distinct from soft
	// delete, it scales the workspace and every child Environment's tenant
	// workloads to zero but touches no data (PVC, namespaces, deployed
	// resources all survive) and can be cleared at any time to resume.
	Suspended bool `json:"suspended,omitempty"`
}

type ProjectTool struct {
	Name    string `json:"name"`
	Version string `json:"version,omitempty"`
}

type ProjectStorage struct {
	Size resource.Quantity `json:"size"`
}

type ProjectStatus struct {
	Phase              string `json:"phase,omitempty"`
	WorkspacePodName   string `json:"workspacePodName,omitempty"`
	ServiceAccountName string `json:"serviceAccountName,omitempty"`
	AgentPath          string `json:"agentPath,omitempty"`
	// RunningWorkspaceImage is the workspace container image currently deployed.
	RunningWorkspaceImage string `json:"runningWorkspaceImage,omitempty"`
	// DesiredWorkspaceImage is the workspace container image the operator wants to run.
	DesiredWorkspaceImage string             `json:"desiredWorkspaceImage,omitempty"`
	Conditions            []metav1.Condition `json:"conditions,omitempty"`
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
	Namespace string `json:"namespace,omitempty"`
	// Subdomain is the environment's stable, randomly-generated single DNS label.
	// The platform serving host is <subdomain>.<deploy zone> (e.g.
	// k7m2x9qf4r.apps.enzarb.dev), so a single wildcard (*.apps.enzarb.dev)
	// resolves every environment. Generated once and never changed.
	Subdomain  string             `json:"subdomain,omitempty"`
	Domains    []DomainStatus     `json:"domains,omitempty"`
	Conditions []metav1.Condition `json:"conditions,omitempty"`
}

type DomainStatus struct {
	FQDN string `json:"fqdn"`
	// CertStatus doubles as the domain's verification phase:
	// PendingVerification | VerificationError | DomainConflict | Verified.
	CertStatus string `json:"certStatus,omitempty"`
	VerifiedAt string `json:"verifiedAt,omitempty"`
	// ChallengeToken is the per-domain secret the tenant must publish as a TXT
	// record at _enzarb-challenge.<fqdn> (value "enzarb-verify=<token>") to prove
	// DNS control. Surfaced to the user so they can create the record.
	ChallengeToken string `json:"challengeToken,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:resource:scope=Namespaced,shortName=alloweddomain
// +kubebuilder:printcolumn:name="FQDNs",type=string,JSONPath=`.spec.fqdns`

// AllowedDomains is the operator-maintained, per-deploy-namespace source of
// truth for which hostnames a project's tenant-authored Gateway API routes
// (HTTPRoute/GRPCRoute) and Ingresses are permitted to claim. The operator
// projects one object (named "default") into each deploy namespace from the
// Environment's verified domains; a ValidatingAdmissionPolicy paramRefs it to
// reject routes whose hostnames fall outside this set. Tenants must not have
// write access to this resource.
type AllowedDomains struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec AllowedDomainsSpec `json:"spec,omitempty"`
}

type AllowedDomainsSpec struct {
	// FQDNs is the exact set of hostnames permitted in this namespace. A
	// wildcard entry ("*.example.com") additionally permits any single-label
	// subdomain of example.com.
	FQDNs []string `json:"fqdns,omitempty"`
}

// +kubebuilder:object:root=true

type AllowedDomainsList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []AllowedDomains `json:"items"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Cluster,shortName=domainclaim
// +kubebuilder:printcolumn:name="FQDN",type=string,JSONPath=`.spec.fqdn`
// +kubebuilder:printcolumn:name="Project",type=string,JSONPath=`.spec.projectRef`
// +kubebuilder:printcolumn:name="Verified",type=string,JSONPath=`.status.verifiedAt`

// DomainClaim is the cluster-scoped ownership ledger for custom domains. Its
// metadata.name is a hash of the FQDN, so etcd's name uniqueness guarantees a
// given hostname can be bound to exactly one project: the operator Creates the
// claim only after DNS ownership is proven, and a second project's Create of the
// same name fails, hard-blocking domain hijacking regardless of route
// creationTimestamp ordering.
type DomainClaim struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   DomainClaimSpec   `json:"spec,omitempty"`
	Status DomainClaimStatus `json:"status,omitempty"`
}

type DomainClaimSpec struct {
	FQDN string `json:"fqdn"`
	// OrgID, ProjectRef and Namespace identify the owning project. Re-verification
	// and route admission are only honored for the project recorded here.
	OrgID      string `json:"orgID"`
	ProjectRef string `json:"projectRef"`
	Namespace  string `json:"namespace"`
}

type DomainClaimStatus struct {
	VerifiedAt string `json:"verifiedAt,omitempty"`
}

// +kubebuilder:object:root=true

type DomainClaimList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []DomainClaim `json:"items"`
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

func (o *Organization) DeepCopyObject() runtime.Object {
	if o == nil {
		return nil
	}
	out := new(Organization)
	o.DeepCopyInto(out)
	return out
}

func (o *Organization) DeepCopyInto(out *Organization) {
	*out = *o
	out.TypeMeta = o.TypeMeta
	o.ObjectMeta.DeepCopyInto(&out.ObjectMeta)
	out.Spec = o.Spec
	o.Status.DeepCopyInto(&out.Status)
}

func (ol *OrganizationList) DeepCopyObject() runtime.Object {
	if ol == nil {
		return nil
	}
	out := new(OrganizationList)
	ol.DeepCopyInto(out)
	return out
}

func (ol *OrganizationList) DeepCopyInto(out *OrganizationList) {
	*out = *ol
	out.TypeMeta = ol.TypeMeta
	ol.ListMeta.DeepCopyInto(&out.ListMeta)
	if ol.Items != nil {
		out.Items = make([]Organization, len(ol.Items))
		for i := range ol.Items {
			ol.Items[i].DeepCopyInto(&out.Items[i])
		}
	}
}

func (s *OrganizationStatus) DeepCopyInto(out *OrganizationStatus) {
	*out = *s
	if s.Conditions != nil {
		out.Conditions = make([]metav1.Condition, len(s.Conditions))
		copy(out.Conditions, s.Conditions)
	}
}

func (a *AllowedDomains) DeepCopyObject() runtime.Object {
	if a == nil {
		return nil
	}
	out := new(AllowedDomains)
	a.DeepCopyInto(out)
	return out
}

func (a *AllowedDomains) DeepCopyInto(out *AllowedDomains) {
	*out = *a
	out.TypeMeta = a.TypeMeta
	a.ObjectMeta.DeepCopyInto(&out.ObjectMeta)
	if a.Spec.FQDNs != nil {
		out.Spec.FQDNs = make([]string, len(a.Spec.FQDNs))
		copy(out.Spec.FQDNs, a.Spec.FQDNs)
	}
}

func (al *AllowedDomainsList) DeepCopyObject() runtime.Object {
	if al == nil {
		return nil
	}
	out := new(AllowedDomainsList)
	al.DeepCopyInto(out)
	return out
}

func (al *AllowedDomainsList) DeepCopyInto(out *AllowedDomainsList) {
	*out = *al
	out.TypeMeta = al.TypeMeta
	al.ListMeta.DeepCopyInto(&out.ListMeta)
	if al.Items != nil {
		out.Items = make([]AllowedDomains, len(al.Items))
		for i := range al.Items {
			al.Items[i].DeepCopyInto(&out.Items[i])
		}
	}
}

func (d *DomainClaim) DeepCopyObject() runtime.Object {
	if d == nil {
		return nil
	}
	out := new(DomainClaim)
	d.DeepCopyInto(out)
	return out
}

func (d *DomainClaim) DeepCopyInto(out *DomainClaim) {
	*out = *d
	out.TypeMeta = d.TypeMeta
	d.ObjectMeta.DeepCopyInto(&out.ObjectMeta)
	out.Spec = d.Spec
	out.Status = d.Status
}

func (dl *DomainClaimList) DeepCopyObject() runtime.Object {
	if dl == nil {
		return nil
	}
	out := new(DomainClaimList)
	dl.DeepCopyInto(out)
	return out
}

func (dl *DomainClaimList) DeepCopyInto(out *DomainClaimList) {
	*out = *dl
	out.TypeMeta = dl.TypeMeta
	dl.ListMeta.DeepCopyInto(&out.ListMeta)
	if dl.Items != nil {
		out.Items = make([]DomainClaim, len(dl.Items))
		for i := range dl.Items {
			dl.Items[i].DeepCopyInto(&out.Items[i])
		}
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
