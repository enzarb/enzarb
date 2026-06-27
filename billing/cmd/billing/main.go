package main

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

func main() {
	slog.Info("billing run starting")

	dbURL := os.Getenv("DATABASE_URL")
	if dbURL == "" {
		slog.Error("DATABASE_URL required")
		os.Exit(1)
	}

	ctx := context.Background()
	pool, err := pgxpool.New(ctx, dbURL)
	if err != nil {
		slog.Error("db connect", "err", err)
		os.Exit(1)
	}
	defer pool.Close()

	now := time.Now().UTC()
	periodStart := time.Date(now.Year(), now.Month()-1, 1, 0, 0, 0, 0, time.UTC)
	periodEnd := time.Date(now.Year(), now.Month(), 1, 0, 0, 0, 0, time.UTC)

	pricing, err := pricingFromDB(ctx, pool)
	if err != nil {
		slog.Error("pricing config", "err", err)
		os.Exit(1)
	}

	rows, err := pool.Query(ctx, `SELECT id, slug, tier FROM organizations`)
	if err != nil {
		slog.Error("list orgs", "err", err)
		os.Exit(1)
	}
	defer rows.Close()

	type org struct {
		ID   string
		Slug string
		Tier string
	}
	var orgs []org
	for rows.Next() {
		var o org
		if err := rows.Scan(&o.ID, &o.Slug, &o.Tier); err != nil {
			slog.Error("scan org", "err", err)
			os.Exit(1)
		}
		orgs = append(orgs, o)
	}
	rows.Close()

	for _, o := range orgs {
		if err := generateOrgInvoice(ctx, pool, o.ID, o.Slug, o.Tier, periodStart, periodEnd, pricing); err != nil {
			slog.Error("generate org invoice", "org", o.Slug, "err", err)
		}
	}

	slog.Info("billing run complete", "period_start", periodStart, "period_end", periodEnd, "orgs", len(orgs))
}

type PricingConfig struct {
	CPUSecondsPerUnit           float64
	MemGiBSecondsPerUnit        float64
	StorageGiBSecondsPerUnit    float64
	ZotStorageGiBSecondsPerUnit float64
	// Network is metered as four independent line items (internal vs external,
	// ingress vs egress), each priced per GiB.
	NetIngressInternalPerGiB float64
	NetEgressInternalPerGiB  float64
	NetIngressExternalPerGiB float64
	NetEgressExternalPerGiB  float64
	// Free-tier monthly allowances, one per billed metric. Compute/storage are
	// in the metric's native unit; network allowances are in GiB.
	FreeCPUSeconds            float64
	FreeMemGiBSeconds         float64
	FreeStorageGiBSeconds     float64
	FreeZotStorageGiBSeconds  float64
	FreeNetIngressInternalGiB float64
	FreeNetEgressInternalGiB  float64
	FreeNetIngressExternalGiB float64
	FreeNetEgressExternalGiB  float64
}

func pricingFromDB(ctx context.Context, db *pgxpool.Pool) (PricingConfig, error) {
	rows, err := db.Query(ctx, `SELECT key, value FROM app_settings WHERE key LIKE 'pricing_%'`)
	if err != nil {
		return PricingConfig{}, fmt.Errorf("load settings: %w", err)
	}
	defer rows.Close()

	values := map[string]string{}
	for rows.Next() {
		var k, v string
		if err := rows.Scan(&k, &v); err != nil {
			return PricingConfig{}, err
		}
		values[k] = v
	}
	if err := rows.Err(); err != nil {
		return PricingConfig{}, err
	}

	var errs []error
	parse := func(key string) float64 {
		v, ok := values[key]
		if !ok {
			errs = append(errs, fmt.Errorf("missing setting %s", key))
			return 0
		}
		f, err := strconv.ParseFloat(v, 64)
		if err != nil {
			errs = append(errs, fmt.Errorf("invalid %s=%q: %w", key, v, err))
		}
		return f
	}

	p := PricingConfig{
		CPUSecondsPerUnit:           parse("pricing_cpu_seconds_per_unit"),
		MemGiBSecondsPerUnit:        parse("pricing_mem_gib_seconds_per_unit"),
		StorageGiBSecondsPerUnit:    parse("pricing_storage_gib_seconds_per_unit"),
		ZotStorageGiBSecondsPerUnit: parse("pricing_zot_storage_gib_seconds_per_unit"),
		NetIngressInternalPerGiB:    parse("pricing_net_ingress_internal_per_gib"),
		NetEgressInternalPerGiB:     parse("pricing_net_egress_internal_per_gib"),
		NetIngressExternalPerGiB:    parse("pricing_net_ingress_external_per_gib"),
		NetEgressExternalPerGiB:     parse("pricing_net_egress_external_per_gib"),
		FreeCPUSeconds:              parse("pricing_free_cpu_seconds"),
		FreeMemGiBSeconds:           parse("pricing_free_mem_gib_seconds"),
		FreeStorageGiBSeconds:       parse("pricing_free_storage_gib_seconds"),
		FreeZotStorageGiBSeconds:    parse("pricing_free_zot_storage_gib_seconds"),
		FreeNetIngressInternalGiB:   parse("pricing_free_net_ingress_internal_gib"),
		FreeNetEgressInternalGiB:    parse("pricing_free_net_egress_internal_gib"),
		FreeNetIngressExternalGiB:   parse("pricing_free_net_ingress_external_gib"),
		FreeNetEgressExternalGiB:    parse("pricing_free_net_egress_external_gib"),
	}
	if len(errs) > 0 {
		return PricingConfig{}, fmt.Errorf("%v", errs)
	}
	return p, nil
}

func generateOrgInvoice(ctx context.Context, db *pgxpool.Pool, orgID, orgSlug, tier string, start, end time.Time, p PricingConfig) error {
	var existing int
	err := db.QueryRow(ctx, `
		SELECT COUNT(*) FROM invoices
		WHERE org_id = $1 AND period_start = $2 AND period_end = $3
	`, orgID, start, end).Scan(&existing)
	if err != nil || existing > 0 {
		return err
	}

	type usageRow struct {
		ResourceType string
		Total        float64
	}
	rows, err := db.Query(ctx, `
		SELECT resource_type, SUM(quantity)
		FROM usage_events
		WHERE org_id = $1 AND recorded_at >= $2 AND recorded_at < $3
		GROUP BY resource_type
	`, orgID, start, end)
	if err != nil {
		return fmt.Errorf("aggregate usage: %w", err)
	}
	defer rows.Close()

	usage := map[string]float64{}
	for rows.Next() {
		var r usageRow
		if err := rows.Scan(&r.ResourceType, &r.Total); err != nil {
			return err
		}
		usage[r.ResourceType] = r.Total
	}
	rows.Close()

	const bytesPerGiB = 1 << 30

	// billableCents applies the metric's free allowance before pricing. Native
	// returns the chargeable cents for a metric measured in its own unit.
	native := func(used, free, perUnit float64) int64 {
		if b := used - free; b > 0 {
			return int64(b * perUnit * 100)
		}
		return 0
	}
	// netCents converts a byte total to GiB, deducts the GiB free allowance,
	// then prices the remainder.
	netCents := func(usedBytes, freeGiB, perGiB float64) int64 {
		if b := usedBytes/bytesPerGiB - freeGiB; b > 0 {
			return int64(b * perGiB * 100)
		}
		return 0
	}

	var totalCents int64
	totalCents += native(usage["cpu_seconds"], p.FreeCPUSeconds, p.CPUSecondsPerUnit)
	totalCents += native(usage["mem_gib_seconds"], p.FreeMemGiBSeconds, p.MemGiBSecondsPerUnit)
	totalCents += native(usage["storage_gib_seconds"], p.FreeStorageGiBSeconds, p.StorageGiBSecondsPerUnit)
	totalCents += native(usage["zot_storage_gib_seconds"], p.FreeZotStorageGiBSeconds, p.ZotStorageGiBSecondsPerUnit)
	totalCents += netCents(usage["net_ingress_internal_bytes"], p.FreeNetIngressInternalGiB, p.NetIngressInternalPerGiB)
	totalCents += netCents(usage["net_egress_internal_bytes"], p.FreeNetEgressInternalGiB, p.NetEgressInternalPerGiB)
	totalCents += netCents(usage["net_ingress_external_bytes"], p.FreeNetIngressExternalGiB, p.NetIngressExternalPerGiB)
	totalCents += netCents(usage["net_egress_external_bytes"], p.FreeNetEgressExternalGiB, p.NetEgressExternalPerGiB)

	if totalCents < 0 {
		totalCents = 0
	}

	_, err = db.Exec(ctx, `
		INSERT INTO invoices (org_id, period_start, period_end, total_cents, status)
		VALUES ($1, $2, $3, $4, 'draft')
	`, orgID, start, end, totalCents)
	if err != nil {
		return fmt.Errorf("insert invoice: %w", err)
	}

	slog.Info("invoice created", "org", orgSlug, "period_start", start, "total_cents", totalCents)
	return nil
}
