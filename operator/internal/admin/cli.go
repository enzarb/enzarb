package admin

import (
	"context"
	"fmt"
	"os"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	utilruntime "k8s.io/apimachinery/pkg/util/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	"k8s.io/client-go/tools/clientcmd"
	"sigs.k8s.io/controller-runtime/pkg/client"
)

var scheme = runtime.NewScheme()

func init() {
	utilruntime.Must(clientgoscheme.AddToScheme(scheme))
	utilruntime.Must(enzarbv1alpha1.AddToScheme(scheme))
}

func newClient() (client.Client, error) {
	rules := clientcmd.NewDefaultClientConfigLoadingRules()
	cfg, err := clientcmd.NewNonInteractiveDeferredLoadingClientConfig(rules, nil).ClientConfig()
	if err != nil {
		return nil, err
	}
	return client.New(cfg, client.Options{Scheme: scheme})
}

// Run dispatches admin subcommands: create-org, set-tier, list-orgs, create-project.
func Run(args []string) {
	if len(args) == 0 {
		fmt.Fprintln(os.Stderr, "Usage: enzarb admin <create-org|set-tier|list-orgs|create-project|set-gpu> [flags]")
		os.Exit(1)
	}

	switch args[0] {
	case "create-org":
		runCreateOrg(args[1:])
	case "set-tier":
		runSetTier(args[1:])
	case "list-orgs":
		runListOrgs()
	case "create-project":
		runCreateProject(args[1:])
	case "set-gpu":
		runSetGPU(args[1:])
	default:
		fmt.Fprintf(os.Stderr, "unknown admin command: %s\n", args[0])
		os.Exit(1)
	}
}

func runCreateOrg(args []string) {
	if len(args) < 2 {
		fmt.Fprintln(os.Stderr, "Usage: enzarb admin create-org <slug> <display-name> [tier]")
		os.Exit(1)
	}
	slug := args[0]
	displayName := args[1]
	tier := "free"
	if len(args) >= 3 {
		tier = args[2]
	}

	// Admin org creation writes to DB — for now print instructions since
	// the DB is managed by the SvelteKit app. This CLI is for K8s CRD ops.
	fmt.Printf("Create org: slug=%s display_name=%q tier=%s\n", slug, displayName, tier)
	fmt.Println("Note: run `INSERT INTO organizations (slug, display_name, tier) VALUES (...)` on the enzarb PostgreSQL cluster.")
	_ = tier
}

func runSetTier(args []string) {
	if len(args) < 2 {
		fmt.Fprintln(os.Stderr, "Usage: enzarb admin set-tier <org-slug> <tier>")
		os.Exit(1)
	}
	fmt.Printf("Set tier: org=%s tier=%s\n", args[0], args[1])
	fmt.Println("Note: run `UPDATE organizations SET tier=$1 WHERE slug=$2` on the enzarb PostgreSQL cluster.")
}

func runListOrgs() {
	c, err := newClient()
	if err != nil {
		fmt.Fprintf(os.Stderr, "k8s client: %v\n", err)
		os.Exit(1)
	}
	ctx := context.Background()
	var projects enzarbv1alpha1.ProjectList
	if err := c.List(ctx, &projects); err != nil {
		fmt.Fprintf(os.Stderr, "list projects: %v\n", err)
		os.Exit(1)
	}

	orgs := map[string]int{}
	for _, p := range projects.Items {
		orgs[p.Spec.OrgID]++
	}
	if len(orgs) == 0 {
		fmt.Println("No orgs with projects found.")
		return
	}
	fmt.Printf("%-30s %s\n", "OrgID", "Projects")
	for org, count := range orgs {
		fmt.Printf("%-30s %d\n", org, count)
	}
}

func runCreateProject(args []string) {
	if len(args) < 3 {
		fmt.Fprintln(os.Stderr, "Usage: enzarb admin create-project <org-id> <slug> <display-name>")
		os.Exit(1)
	}
	orgID := args[0]
	slug := args[1]
	displayName := args[2]

	c, err := newClient()
	if err != nil {
		fmt.Fprintf(os.Stderr, "k8s client: %v\n", err)
		os.Exit(1)
	}

	project := &enzarbv1alpha1.Project{
		ObjectMeta: metav1.ObjectMeta{
			Name:      slug,
			Namespace: fmt.Sprintf("user-%s", orgID),
		},
		Spec: enzarbv1alpha1.ProjectSpec{
			OrgID:       orgID,
			Slug:        slug,
			DisplayName: displayName,
			Storage: enzarbv1alpha1.ProjectStorage{
				Size: resource.MustParse("10Gi"),
			},
		},
	}

	ctx := context.Background()
	if err := c.Create(ctx, project); err != nil {
		fmt.Fprintf(os.Stderr, "create project: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Created project %s/%s\n", orgID, slug)
}

func runSetGPU(args []string) {
	if len(args) < 3 {
		fmt.Fprintln(os.Stderr, "Usage: enzarb admin set-gpu <org-id> <project-slug> <true|false>")
		os.Exit(1)
	}
	orgID := args[0]
	slug := args[1]
	enable := args[2] == "true"

	c, err := newClient()
	if err != nil {
		fmt.Fprintf(os.Stderr, "k8s client: %v\n", err)
		os.Exit(1)
	}

	ctx := context.Background()
	ns := fmt.Sprintf("user-%s", orgID)

	var project enzarbv1alpha1.Project
	if err := c.Get(ctx, types.NamespacedName{Name: slug, Namespace: ns}, &project); err != nil {
		fmt.Fprintf(os.Stderr, "get project: %v\n", err)
		os.Exit(1)
	}

	project.Spec.GPUEnabled = enable
	if err := c.Update(ctx, &project); err != nil {
		fmt.Fprintf(os.Stderr, "update project: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Set gpuEnabled=%v on project %s/%s\n", enable, orgID, slug)
}
