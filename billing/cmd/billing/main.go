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
	NetIngressPerGiB            float64
	NetEgressPerGiB             float64
	StorageGiBSecondsPerUnit    float64
	ZotStorageGiBSecondsPerUnit float64
	FreeCPUSeconds              float64
	FreeMemGiBSeconds           float64
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
		NetIngressPerGiB:            parse("pricing_net_ingress_per_gib"),
		NetEgressPerGiB:             parse("pricing_net_egress_per_gib"),
		StorageGiBSecondsPerUnit:    parse("pricing_storage_gib_seconds_per_unit"),
		ZotStorageGiBSecondsPerUnit: parse("pricing_zot_storage_gib_seconds_per_unit"),
		FreeCPUSeconds:              parse("pricing_free_cpu_seconds"),
		FreeMemGiBSeconds:           parse("pricing_free_mem_gib_seconds"),
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

	var totalCents int64

	if cpuBillable := usage["cpu_seconds"] - p.FreeCPUSeconds; cpuBillable > 0 {
		totalCents += int64(cpuBillable * p.CPUSecondsPerUnit * 100)
	}
	if memBillable := usage["mem_gib_seconds"] - p.FreeMemGiBSeconds; memBillable > 0 {
		totalCents += int64(memBillable * p.MemGiBSecondsPerUnit * 100)
	}

	const bytesPerGiB = 1 << 30
	totalCents += int64(usage["net_ingress_bytes"] / bytesPerGiB * p.NetIngressPerGiB * 100)
	totalCents += int64(usage["net_egress_bytes"] / bytesPerGiB * p.NetEgressPerGiB * 100)
	totalCents += int64(usage["storage_gib_seconds"] * p.StorageGiBSecondsPerUnit * 100)
	totalCents += int64(usage["zot_storage_gib_seconds"] * p.ZotStorageGiBSecondsPerUnit * 100)

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
