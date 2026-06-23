package controller

import (
	"context"
	"fmt"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"
)

const managedByLabel = "app.kubernetes.io/managed-by"
const managedByValue = "enzarb-operator"

// pruneUnmanaged deletes operator-managed objects whose names are not in
// expected. It lists all objects of type L with the standard managed-by label,
// then deletes any not present in expected.
//
// Pass a non-empty namespace to scope the list to a single namespace.
// Pass "" for cluster-scoped resource types.
//
// This handles forward-compatibility: when the operator stops managing a
// resource type (or renames a resource), stale copies left from a prior
// version are removed on the next reconcile rather than accumulating silently.
//
// Pass a nil or empty expected map to delete every managed object of that type.
func pruneUnmanaged[T client.Object, L interface {
	client.ObjectList
}](
	ctx context.Context,
	c client.Client,
	list L,
	namespace string,
	expected map[string]struct{},
	getItems func(L) []T,
) error {
	logger := log.FromContext(ctx)

	listOpts := []client.ListOption{client.MatchingLabels{managedByLabel: managedByValue}}
	if namespace != "" {
		listOpts = append(listOpts, client.InNamespace(namespace))
	}
	if err := c.List(ctx, list, listOpts...); err != nil {
		return fmt.Errorf("list: %w", err)
	}

	for _, obj := range getItems(list) {
		name := obj.GetName()
		if _, ok := expected[name]; ok {
			continue
		}
		gvk := obj.GetObjectKind().GroupVersionKind()
		logger.Info("pruning unmanaged resource", "kind", gvk.Kind, "namespace", namespace, "name", name)
		if err := c.Delete(ctx, obj); err != nil && !apierrors.IsNotFound(err) {
			return fmt.Errorf("delete %s %s: %w", gvk.Kind, name, err)
		}
	}
	return nil
}
