package main

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/riverqueue/river"
	"github.com/riverqueue/river/riverdriver/riverpgxv5"
)

func main() {
	slog.Info("billing worker starting")

	dbURL := os.Getenv("DATABASE_URL")
	if dbURL == "" {
		slog.Error("DATABASE_URL required")
		os.Exit(1)
	}

	pool, err := pgxpool.New(context.Background(), dbURL)
	if err != nil {
		slog.Error("db connect", "err", err)
		os.Exit(1)
	}
	defer pool.Close()

	workers := river.NewWorkers()
	river.AddWorker(workers, &InvoiceWorker{db: pool})

	riverClient, err := river.NewClient(riverpgxv5.New(pool), &river.Config{
		Queues: map[string]river.QueueConfig{
			river.QueueDefault: {MaxWorkers: 2},
		},
		Workers: workers,
	})
	if err != nil {
		slog.Error("river client", "err", err)
		os.Exit(1)
	}

	ctx := context.Background()

	// Schedule monthly invoice generation if not already scheduled
	if err := scheduleMonthlyInvoice(ctx, riverClient); err != nil {
		slog.Warn("schedule invoice", "err", err)
	}

	if err := riverClient.Start(ctx); err != nil {
		slog.Error("river start", "err", err)
		os.Exit(1)
	}
}

// InvoiceArgs holds arguments for the invoice generation job.
type InvoiceArgs struct {
	PeriodStart time.Time `json:"period_start"`
	PeriodEnd   time.Time `json:"period_end"`
}

func (InvoiceArgs) Kind() string { return "generate_invoices" }

// InvoiceWorker aggregates usage_events into invoices with tiered pricing.
type InvoiceWorker struct {
	db *pgxpool.Pool
	river.WorkerDefaults[InvoiceArgs]
}

func (w *InvoiceWorker) Work(ctx context.Context, job *river.Job[InvoiceArgs]) error {
	start := job.Args.PeriodStart
	end := job.Args.PeriodEnd
	slog.Info("generating invoices", "period_start", start, "period_end", end)

	// Get pricing config from enzarb-config (stored in DB as config table or env)
	pricing := defaultPricing()

	// Get all orgs
	rows, err := w.db.Query(ctx, `SELECT id, slug, tier FROM organizations`)
	if err != nil {
		return fmt.Errorf("list orgs: %w", err)
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
			return err
		}
		orgs = append(orgs, o)
	}
	rows.Close()

	for _, o := range orgs {
		if err := w.generateOrgInvoice(ctx, o.ID, o.Slug, o.Tier, start, end, pricing); err != nil {
			slog.Error("generate org invoice", "org", o.Slug, "err", err)
		}
	}
	return nil
}

type PricingConfig struct {
	CPUSecondsPerUnit        float64
	MemGiBSecondsPerUnit     float64
	NetIngressPerByte        float64
	NetEgressPerByte         float64
	StorageGiBSecondsPerUnit float64
	FreeCPUSeconds           float64
	FreeMemGiBSeconds        float64
}

func defaultPricing() PricingConfig {
	return PricingConfig{
		CPUSecondsPerUnit:    0.0000139,
		MemGiBSecondsPerUnit: 0.0000028,
		NetIngressPerByte:    0.0000000001,
		NetEgressPerByte:     0.0000000009,
		StorageGiBSecondsPerUnit: 0.0000000385,
		FreeCPUSeconds:       36000,
		FreeMemGiBSeconds:    107374182,
	}
}

func (w *InvoiceWorker) generateOrgInvoice(ctx context.Context, orgID, orgSlug, tier string, start, end time.Time, p PricingConfig) error {
	// Check for existing invoice for this period
	var existing int
	err := w.db.QueryRow(ctx, `
		SELECT COUNT(*) FROM invoices
		WHERE org_id = $1 AND period_start = $2 AND period_end = $3
	`, orgID, start, end).Scan(&existing)
	if err != nil || existing > 0 {
		return err
	}

	// Aggregate usage by resource type
	type usageRow struct {
		ResourceType string
		Total        float64
	}
	rows, err := w.db.Query(ctx, `
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

	// Calculate total in cents
	var totalCents int64

	cpuBillable := usage["cpu_seconds"] - p.FreeCPUSeconds
	if cpuBillable > 0 {
		totalCents += int64(cpuBillable * p.CPUSecondsPerUnit * 100)
	}

	memBillable := usage["mem_gib_seconds"] - p.FreeMemGiBSeconds
	if memBillable > 0 {
		totalCents += int64(memBillable * p.MemGiBSecondsPerUnit * 100)
	}

	totalCents += int64(usage["net_ingress_bytes"] * p.NetIngressPerByte * 100)
	totalCents += int64(usage["net_egress_bytes"] * p.NetEgressPerByte * 100)
	totalCents += int64(usage["storage_gib_seconds"] * p.StorageGiBSecondsPerUnit * 100)

	if totalCents < 0 {
		totalCents = 0
	}

	_, err = w.db.Exec(ctx, `
		INSERT INTO invoices (org_id, period_start, period_end, total_cents, status)
		VALUES ($1, $2, $3, $4, 'draft')
	`, orgID, start, end, totalCents)
	if err != nil {
		return fmt.Errorf("insert invoice: %w", err)
	}

	slog.Info("invoice created", "org", orgSlug, "period_start", start, "total_cents", totalCents)
	return nil
}

func scheduleMonthlyInvoice(ctx context.Context, client *river.Client[pgx.Tx]) error { //nolint:unparam
	// Schedule for the 1st of next month
	now := time.Now().UTC()
	firstOfNextMonth := time.Date(now.Year(), now.Month()+1, 1, 0, 0, 0, 0, time.UTC)
	periodStart := time.Date(now.Year(), now.Month(), 1, 0, 0, 0, 0, time.UTC)

	_, err := client.Insert(ctx, &InvoiceArgs{
		PeriodStart: periodStart,
		PeriodEnd:   firstOfNextMonth,
	}, &river.InsertOpts{
		ScheduledAt: firstOfNextMonth,
	})
	return err
}
