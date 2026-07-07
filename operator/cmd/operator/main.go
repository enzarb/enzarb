package main

import (
	"flag"
	"os"

	certmanagerv1 "github.com/cert-manager/cert-manager/pkg/apis/certmanager/v1"
	"k8s.io/apimachinery/pkg/runtime"
	utilruntime "k8s.io/apimachinery/pkg/util/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/healthz"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
	gatewayv1 "sigs.k8s.io/gateway-api/apis/v1"
	gatewayv1beta1 "sigs.k8s.io/gateway-api/apis/v1beta1"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
	"enzarb.dev/enzarb/operator/internal/admin"
	"enzarb.dev/enzarb/operator/internal/controller"
)

var (
	scheme   = runtime.NewScheme()
	setupLog = ctrl.Log.WithName("setup")
)

func init() {
	utilruntime.Must(clientgoscheme.AddToScheme(scheme))
	utilruntime.Must(enzarbv1alpha1.AddToScheme(scheme))
	utilruntime.Must(gatewayv1.Install(scheme))
	utilruntime.Must(gatewayv1beta1.Install(scheme))
	utilruntime.Must(certmanagerv1.AddToScheme(scheme))
}

func main() {
	// admin subcommand: enzarb admin <cmd> [args...]
	if len(os.Args) > 1 && os.Args[1] == "admin" {
		admin.Run(os.Args[2:])
		return
	}

	var metricsAddr string
	var probeAddr string
	var enableLeaderElection bool

	flag.StringVar(&metricsAddr, "metrics-bind-address", ":8080", "Address for metrics endpoint")
	flag.StringVar(&probeAddr, "health-probe-bind-address", ":8081", "Address for health probe endpoint")
	flag.BoolVar(&enableLeaderElection, "leader-elect", false, "Enable leader election")

	opts := zap.Options{Development: true}
	opts.BindFlags(flag.CommandLine)
	flag.Parse()

	ctrl.SetLogger(zap.New(zap.UseFlagOptions(&opts)))

	mgr, err := ctrl.NewManager(ctrl.GetConfigOrDie(), ctrl.Options{
		Scheme:                 scheme,
		HealthProbeBindAddress: probeAddr,
		LeaderElection:         enableLeaderElection,
		LeaderElectionID:       "enzarb-operator-lock",
	})
	if err != nil {
		setupLog.Error(err, "unable to create manager")
		os.Exit(1)
	}

	domain := os.Getenv("ENZARB_DOMAIN")
	if domain == "" {
		domain = "enzarb.dev"
	}

	// NETWORK_POLICY_ENABLED=false is a single switch that disables all
	// workspace and deploy-namespace network isolation at once. It exists for
	// clusters whose CNI can't enforce NetworkPolicy, but leaving it off in
	// production removes the primary barrier against lateral movement from
	// workspace pods. Make that state loud instead of silent so it can't be
	// flipped unnoticed.
	if os.Getenv("NETWORK_POLICY_ENABLED") == "false" {
		setupLog.Info("SECURITY WARNING: NETWORK_POLICY_ENABLED=false — workspace and deploy-namespace network isolation is DISABLED; workspace pods can reach control-plane and sibling pods at the network layer")
	}

	if err = (&controller.ProjectReconciler{
		Client:    mgr.GetClient(),
		Scheme:    mgr.GetScheme(),
		Domain:    domain,
		APIReader: mgr.GetAPIReader(),
	}).SetupWithManager(mgr); err != nil {
		setupLog.Error(err, "unable to create ProjectReconciler")
		os.Exit(1)
	}

	if err = (&controller.OrganizationReconciler{
		Client:    mgr.GetClient(),
		Scheme:    mgr.GetScheme(),
		APIReader: mgr.GetAPIReader(),
	}).SetupWithManager(mgr); err != nil {
		setupLog.Error(err, "unable to create OrganizationReconciler")
		os.Exit(1)
	}

	if err = (&controller.EnvironmentReconciler{
		Client:    mgr.GetClient(),
		Scheme:    mgr.GetScheme(),
		APIReader: mgr.GetAPIReader(),
	}).SetupWithManager(mgr); err != nil {
		setupLog.Error(err, "unable to create EnvironmentReconciler")
		os.Exit(1)
	}

	if err := mgr.AddHealthzCheck("healthz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up health check")
		os.Exit(1)
	}
	if err := mgr.AddReadyzCheck("readyz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up ready check")
		os.Exit(1)
	}

	setupLog.Info("starting manager")
	if err := mgr.Start(ctrl.SetupSignalHandler()); err != nil {
		setupLog.Error(err, "problem running manager")
		os.Exit(1)
	}
}
