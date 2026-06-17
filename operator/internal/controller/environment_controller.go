package controller

import (
	"context"
	"fmt"

	corev1 "k8s.io/api/core/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"

	enzarbv1alpha1 "enzarb.dev/enzarb/operator/api/v1alpha1"
)

type EnvironmentReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

func (r *EnvironmentReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&enzarbv1alpha1.Environment{}).
		Complete(r)
}

func (r *EnvironmentReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var env enzarbv1alpha1.Environment
	if err := r.Get(ctx, req.NamespacedName, &env); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Resolve parent project to get org ID
	var project enzarbv1alpha1.Project
	if err := r.Get(ctx, types.NamespacedName{
		Namespace: env.Namespace,
		Name:      env.Spec.ProjectRef.Name,
	}, &project); err != nil {
		return ctrl.Result{}, fmt.Errorf("get project: %w", err)
	}

	deployNS := fmt.Sprintf("deploy-%s-%s", project.Spec.OrgID, env.Spec.Slug)
	orgNS := env.Namespace
	saName := fmt.Sprintf("%s-sa", project.Spec.Slug)

	if err := r.ensureNamespace(ctx, deployNS); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deploy namespace: %w", err)
	}

	if err := r.ensureDeployerRoleBinding(ctx, deployNS, orgNS, saName, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure deployer rolebinding: %w", err)
	}

	if err := r.ensureImagePullRoleBinding(ctx, deployNS, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensure image pull rolebinding: %w", err)
	}

	env.Status.Namespace = deployNS
	if err := r.Status().Update(ctx, &env); err != nil {
		return ctrl.Result{}, fmt.Errorf("update status: %w", err)
	}

	logger.Info("environment reconciled", "name", env.Name, "deployNS", deployNS)
	return ctrl.Result{}, nil
}

func (r *EnvironmentReconciler) ensureDeployerRoleBinding(ctx context.Context, deployNS, orgNS, saName string, env *enzarbv1alpha1.Environment) error {
	name := fmt.Sprintf("enzarb-deployer-%s", env.Spec.ProjectRef.Name)
	rb := &rbacv1.RoleBinding{}
	err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: name}, rb)
	if errors.IsNotFound(err) {
		rb = &rbacv1.RoleBinding{
			ObjectMeta: metav1.ObjectMeta{
				Name:      name,
				Namespace: deployNS,
			},
			RoleRef: rbacv1.RoleRef{
				APIGroup: "rbac.authorization.k8s.io",
				Kind:     "ClusterRole",
				Name:     "enzarb-deployer",
			},
			Subjects: []rbacv1.Subject{
				{Kind: "ServiceAccount", Name: saName, Namespace: orgNS},
			},
		}
		return r.Create(ctx, rb)
	}
	return err
}

func (r *EnvironmentReconciler) ensureImagePullRoleBinding(ctx context.Context, deployNS string, env *enzarbv1alpha1.Environment) error {
	name := "enzarb-image-pull"
	rb := &rbacv1.RoleBinding{}
	err := r.Get(ctx, types.NamespacedName{Namespace: deployNS, Name: name}, rb)
	if errors.IsNotFound(err) {
		rb = &rbacv1.RoleBinding{
			ObjectMeta: metav1.ObjectMeta{
				Name:      name,
				Namespace: deployNS,
			},
			RoleRef: rbacv1.RoleRef{
				APIGroup: "rbac.authorization.k8s.io",
				Kind:     "ClusterRole",
				Name:     "system:image-puller",
			},
			Subjects: []rbacv1.Subject{
				{Kind: "ServiceAccount", Name: "default", Namespace: deployNS},
			},
		}
		return r.Create(ctx, rb)
	}
	return err
}

func (r *EnvironmentReconciler) ensureNamespace(ctx context.Context, name string) error {
	ns := &corev1.Namespace{}
	err := r.Get(ctx, types.NamespacedName{Name: name}, ns)
	if errors.IsNotFound(err) {
		return r.Create(ctx, &corev1.Namespace{
			ObjectMeta: metav1.ObjectMeta{
				Name: name,
				Labels: map[string]string{
					"app.kubernetes.io/managed-by": "enzarb-operator",
					"enzarb.io/type":               "deploy",
				},
			},
		})
	}
	return err
}
