import { PDFDocument, StandardFonts, rgb, type PDFFont, type PDFPage } from 'pdf-lib';
import { RESOURCE_LABELS } from './billing';

export interface InvoiceLineItem {
	resource_type: string;
	quantity: number;
	unit: string;
	unit_price_cents: number;
	amount_cents: number;
}

export interface InvoicePdfData {
	orgName: string;
	invoiceId: string;
	periodStart: Date;
	periodEnd: Date;
	status: string;
	totalCents: number;
	lineItems: InvoiceLineItem[];
	// True when the invoice predates per-line-item storage — line items shown
	// are recomputed at current rates and may not exactly match totalCents.
	estimatedLineItems: boolean;
}

const PAGE_W = 612; // US Letter, points
const PAGE_H = 792;
const MARGIN = 56;

const usd = (cents: number) => '$' + (cents / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const fmtDate = (d: Date) => d.toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' });

// Short, human-readable labels for the unit codes stored on invoice_line_items
// (see billing/cmd/billing/main.go and toDisplayLineItem in billing.remote.ts).
// Anything unrecognized (e.g. a raw resource_type slipping through) is shown
// as-is rather than throwing, since this is a rendering concern only.
const UNIT_LABELS: Record<string, string> = {
	vcpu_hours: 'vCPU-hr',
	gib_hours: 'GiB-hr',
	gib_months: 'GiB-mo',
	gib: 'GiB'
};
const unitLabel = (unit: string) => UNIT_LABELS[unit] ?? unit;

export async function buildInvoicePdf(data: InvoicePdfData): Promise<Uint8Array> {
	const doc = await PDFDocument.create();
	const font = await doc.embedFont(StandardFonts.Helvetica);
	const bold = await doc.embedFont(StandardFonts.HelveticaBold);

	const pages: { page: PDFPage; y: number }[] = [];
	function newPage(): { page: PDFPage; y: number } {
		const page = doc.addPage([PAGE_W, PAGE_H]);
		const entry = { page, y: PAGE_H - MARGIN };
		pages.push(entry);
		return entry;
	}

	let cur = newPage();
	function ensureSpace(needed: number) {
		if (cur.y - needed < MARGIN) cur = newPage();
	}
	function text(str: string, x: number, size: number, f: PDFFont = font, color = rgb(0.1, 0.1, 0.12)) {
		cur.page.drawText(str, { x, y: cur.y, size, font: f, color });
	}
	function line(gap = 18) {
		cur.y -= gap;
	}
	function hr() {
		cur.page.drawLine({
			start: { x: MARGIN, y: cur.y },
			end: { x: PAGE_W - MARGIN, y: cur.y },
			thickness: 0.75,
			color: rgb(0.8, 0.8, 0.82)
		});
	}

	// --- Page 1: summary -------------------------------------------------
	text('Enzarb', MARGIN, 22, bold);
	line(30);
	text('Invoice', MARGIN, 16, bold);
	line(26);
	text(`Organization: ${data.orgName}`, MARGIN, 11);
	line(16);
	text(`Invoice ID: ${data.invoiceId}`, MARGIN, 11);
	line(16);
	text(`Billing period: ${fmtDate(data.periodStart)} – ${fmtDate(data.periodEnd)}`, MARGIN, 11);
	line(16);
	text(`Status: ${data.status}`, MARGIN, 11);
	line(28);
	hr();
	line(30);

	text('Total due', MARGIN, 13, bold);
	text(usd(data.totalCents), PAGE_W - MARGIN - 90, 13, bold);
	line(30);

	if (data.estimatedLineItems) {
		text(
			'Line items below are recomputed at current rates for display; this',
			MARGIN,
			9,
			font,
			rgb(0.5, 0.5, 0.5)
		);
		line(12);
		text(
			'invoice predates per-line-item pricing snapshots and may not sum',
			MARGIN,
			9,
			font,
			rgb(0.5, 0.5, 0.5)
		);
		line(12);
		text('exactly to the total above.', MARGIN, 9, font, rgb(0.5, 0.5, 0.5));
		line(24);
	}

	// --- Line items table (spills onto new pages as needed) --------------
	const colX = { resource: MARGIN, qty: MARGIN + 140, unit: MARGIN + 250, rate: MARGIN + 340, amount: MARGIN + 420 };

	function tableHeader() {
		ensureSpace(40);
		text('Line item detail', MARGIN, 13, bold);
		line(22);
		text('Resource', colX.resource, 10, bold);
		text('Quantity', colX.qty, 10, bold);
		text('Unit', colX.unit, 10, bold);
		text('Rate', colX.rate, 10, bold);
		text('Amount', colX.amount, 10, bold);
		line(8);
		hr();
		line(16);
	}

	tableHeader();

	if (data.lineItems.length === 0) {
		text('No usage recorded for this period.', MARGIN, 10, font, rgb(0.5, 0.5, 0.5));
		line(16);
	}

	for (const li of data.lineItems) {
		ensureSpace(22);
		// A page break mid-table needs its own header for readability.
		if (cur.y === PAGE_H - MARGIN) tableHeader();
		const label = RESOURCE_LABELS[li.resource_type] ?? li.resource_type;
		text(label, colX.resource, 10);
		text(li.quantity.toLocaleString('en-US', { maximumFractionDigits: 4 }), colX.qty, 10);
		text(unitLabel(li.unit), colX.unit, 10);
		text(usd(li.unit_price_cents) + '/u', colX.rate, 10);
		text(usd(li.amount_cents), colX.amount, 10);
		line(18);
	}

	line(10);
	hr();
	line(20);
	text('Total', colX.rate, 12, bold);
	text(usd(data.totalCents), colX.amount, 12, bold);

	// --- Page numbers ------------------------------------------------------
	pages.forEach(({ page }, i) => {
		page.drawText(`Page ${i + 1} of ${pages.length}`, {
			x: PAGE_W - MARGIN - 70,
			y: MARGIN - 30,
			size: 8,
			font,
			color: rgb(0.6, 0.6, 0.6)
		});
	});

	return doc.save();
}
