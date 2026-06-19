package controller

import (
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// purgeAfterAnnotation, when present on a Project or Organization, marks the
// object as soft-deleted: it is retained (workspace scaled to zero, data kept)
// and recoverable until the given RFC3339 timestamp, after which the operator
// hard-deletes it. The app stamps an absolute timestamp (now + retention_days)
// so the operator stays policy-agnostic; recovery just clears the annotation.
const purgeAfterAnnotation = "enzarb.io/purge-after"

// purgeAfter returns the scheduled purge time and whether the object is
// soft-deleted (annotation present and parseable).
func purgeAfter(obj metav1.Object) (time.Time, bool) {
	v := obj.GetAnnotations()[purgeAfterAnnotation]
	if v == "" {
		return time.Time{}, false
	}
	t, err := time.Parse(time.RFC3339, v)
	if err != nil {
		return time.Time{}, false
	}
	return t, true
}
