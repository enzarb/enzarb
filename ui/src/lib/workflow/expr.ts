/**
 * Edge conditions of graph workflows (ADR-0020): a port of `workflows/crates/graph/src/expr.rs`.
 * Keep the two in step; `workflows/crates/graph/tests/fixtures/validate.json` is checked by both.
 *
 * ```text
 * expr    = or
 * or      = and ("||" and)*
 * and     = unary ("&&" unary)*
 * unary   = "!" unary | cmp
 * cmp     = primary (("==" | "!=") primary)?
 * primary = ident | number | string | true | false | null | "(" expr ")"
 * ident   = [A-Za-z_][A-Za-z0-9_]* ("." [A-Za-z_][A-Za-z0-9_]*)*
 * ```
 */

export type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

export type Expr =
	| { t: 'lit'; v: Json }
	| { t: 'ident'; path: string[] }
	| { t: 'not'; e: Expr }
	| { t: 'eq' | 'ne' | 'and' | 'or'; l: Expr; r: Expr };

/** Longest condition accepted, in UTF-8 bytes. */
export const MAX_EXPR_LEN = 512;
const MAX_DEPTH = 32;

type Tok =
	| { k: 'ident'; path: string[] }
	| { k: 'lit'; v: Json }
	| { k: 'Eq' | 'Ne' | 'And' | 'Or' | 'Not' | 'LParen' | 'RParen' };

export class ExprError extends Error {
	constructor(
		readonly expr: string,
		readonly reason: string
	) {
		super(`condition \`${expr}\`: ${reason}`);
		this.name = 'ExprError';
	}
}

/** Rust `{:?}` of a JSON literal, close enough for messages. */
function debugLit(v: Json): string {
	if (v === null) return 'Null';
	if (typeof v === 'boolean') return `Bool(${v})`;
	if (typeof v === 'number') return `Number(${v})`;
	return `String(${JSON.stringify(v)})`;
}

function debugTok(t: Tok): string {
	if (t.k === 'ident') return `Ident([${t.path.map((p) => JSON.stringify(p)).join(', ')}])`;
	if (t.k === 'lit') return `Lit(${debugLit(t.v)})`;
	return t.k;
}

const isDigit = (c: string | undefined) => c !== undefined && c >= '0' && c <= '9';
const isAlpha = (c: string | undefined) =>
	c !== undefined && ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z'));
const isAlnum = (c: string | undefined) => isDigit(c) || isAlpha(c);

/** Operators, longest first where they share a prefix (`!=` before `!`). */
const OPS: [string, 'Eq' | 'Ne' | 'And' | 'Or' | 'Not' | 'LParen' | 'RParen'][] = [
	['(', 'LParen'],
	[')', 'RParen'],
	['==', 'Eq'],
	['!=', 'Ne'],
	['!', 'Not'],
	['&&', 'And'],
	['||', 'Or']
];

function lex(s: string): Tok[] {
	const out: Tok[] = [];
	let i = 0;
	while (i < s.length) {
		const c = s[i];
		const op = OPS.find(([text]) => s.startsWith(text, i));
		if (c === ' ' || c === '\t' || c === '\n' || c === '\r') i++;
		else if (op) {
			out.push({ k: op[1] });
			i += op[0].length;
		} else if (c === '"' || c === "'") {
			let j = i + 1;
			let lit = '';
			for (;;) {
				const x = s[j];
				if (x === undefined) throw 'unterminated string';
				if (x === c) break;
				if (x === '\\') {
					const e = s[j + 1];
					if (e === '\\' || e === '"' || e === "'") lit += e;
					else if (e === 'n') lit += '\n';
					else throw 'bad escape in string';
					j += 2;
				} else {
					const ch = String.fromCodePoint(s.codePointAt(j)!);
					lit += ch;
					j += ch.length;
				}
			}
			out.push({ k: 'lit', v: lit });
			i = j + 1;
		} else if (isDigit(c) || c === '-') {
			const start = i;
			i++;
			while (i < s.length && (isDigit(s[i]) || s[i] === '.')) i++;
			const text = s.slice(start, i);
			let v: number;
			if (/^-?[0-9]+$/.test(text)) v = Number(text);
			else if (/^-?([0-9]+\.?[0-9]*|\.[0-9]+)$/.test(text)) v = Number(text);
			else throw `bad number \`${text}\``;
			if (!Number.isFinite(v)) throw `bad number \`${text}\``;
			out.push({ k: 'lit', v });
		} else if (isAlpha(c) || c === '_') {
			const start = i;
			while (i < s.length && (isAlnum(s[i]) || s[i] === '_' || s[i] === '.')) i++;
			const word = s.slice(start, i);
			if (word === 'true') out.push({ k: 'lit', v: true });
			else if (word === 'false') out.push({ k: 'lit', v: false });
			else if (word === 'null') out.push({ k: 'lit', v: null });
			else {
				const parts = word.split('.');
				if (parts.some((p) => p === '' || isDigit(p[0]))) throw `bad identifier \`${word}\``;
				out.push({ k: 'ident', path: parts });
			}
		} else {
			throw `unexpected character \`${String.fromCodePoint(s.codePointAt(i)!)}\``;
		}
	}
	return out;
}

class Parser {
	pos = 0;
	depth = 0;
	constructor(readonly toks: Tok[]) {}

	peek(): Tok | undefined {
		return this.toks[this.pos];
	}

	eat(k: Tok['k']): boolean {
		if (this.peek()?.k === k) {
			this.pos++;
			return true;
		}
		return false;
	}

	deeper() {
		this.depth++;
		if (this.depth > MAX_DEPTH) throw 'nested too deeply';
	}

	or(): Expr {
		let l = this.and();
		while (this.eat('Or')) l = { t: 'or', l, r: this.and() };
		return l;
	}

	and(): Expr {
		let l = this.unary();
		while (this.eat('And')) l = { t: 'and', l, r: this.unary() };
		return l;
	}

	unary(): Expr {
		if (this.eat('Not')) {
			this.deeper();
			const e = this.unary();
			this.depth--;
			return { t: 'not', e };
		}
		return this.cmp();
	}

	cmp(): Expr {
		const l = this.primary();
		if (this.eat('Eq')) return { t: 'eq', l, r: this.primary() };
		if (this.eat('Ne')) return { t: 'ne', l, r: this.primary() };
		return l;
	}

	primary(): Expr {
		const t = this.peek();
		this.pos++;
		if (!t) throw 'unexpected end';
		if (t.k === 'ident') return { t: 'ident', path: t.path };
		if (t.k === 'lit') return { t: 'lit', v: t.v };
		if (t.k === 'LParen') {
			this.deeper();
			const e = this.or();
			this.depth--;
			if (!this.eat('RParen')) throw 'missing `)`';
			return e;
		}
		throw `unexpected ${debugTok(t)}`;
	}
}

/** Parses a condition; throws {@link ExprError}. */
export function parseExpr(src: string): Expr {
	const fail = (reason: unknown): never => {
		if (typeof reason === 'string') throw new ExprError(src, reason);
		throw reason;
	};
	if (new TextEncoder().encode(src).length > MAX_EXPR_LEN)
		fail(`longer than ${MAX_EXPR_LEN} bytes`);
	let toks: Tok[] = [];
	try {
		toks = lex(src);
	} catch (e) {
		fail(e);
	}
	if (toks.length === 0) fail('empty');
	const p = new Parser(toks);
	let e: Expr = { t: 'lit', v: null };
	try {
		e = p.or();
	} catch (err) {
		fail(err);
	}
	if (p.pos !== toks.length) fail(`unexpected ${debugTok(toks[p.pos])}`);
	return e;
}

/** The parse problem of `src`, or `null` when it parses. */
export function exprProblem(src: string): string | null {
	try {
		parseExpr(src);
		return null;
	} catch (e) {
		if (e instanceof ExprError) return e.message;
		throw e;
	}
}

/** JSON truthiness: `null`, `false`, `0`, `""` and empty arrays/objects are false. */
export function truthy(v: Json | undefined): boolean {
	if (v === null || v === undefined) return false;
	if (typeof v === 'boolean') return v;
	if (typeof v === 'number') return v !== 0;
	if (typeof v === 'string') return v.length > 0;
	if (Array.isArray(v)) return v.length > 0;
	return Object.keys(v).length > 0;
}

function jsonEqual(a: Json, b: Json): boolean {
	if (a === b) return true;
	if (a === null || b === null || typeof a !== 'object' || typeof b !== 'object') return false;
	if (Array.isArray(a) !== Array.isArray(b)) return false;
	if (Array.isArray(a) && Array.isArray(b))
		return a.length === b.length && a.every((x, i) => jsonEqual(x, b[i]));
	const ao = a as Record<string, Json>;
	const bo = b as Record<string, Json>;
	const ka = Object.keys(ao);
	return (
		ka.length === Object.keys(bo).length &&
		ka.every((k) => Object.hasOwn(bo, k) && jsonEqual(ao[k], bo[k]))
	);
}

function value(e: Expr, ctx: Json): Json {
	if (e.t === 'lit') return e.v;
	if (e.t === 'ident') {
		let cur: Json = ctx;
		for (const p of e.path) {
			if (cur === null || typeof cur !== 'object' || Array.isArray(cur) || !Object.hasOwn(cur, p))
				return null;
			cur = cur[p];
		}
		return cur;
	}
	return evalExpr(e, ctx);
}

/** Evaluates `e` against the output of the node that just ran. */
export function evalExpr(e: Expr, ctx: Json): boolean {
	switch (e.t) {
		case 'lit':
		case 'ident':
			return truthy(value(e, ctx));
		case 'not':
			return !evalExpr(e.e, ctx);
		case 'eq':
			return jsonEqual(value(e.l, ctx), value(e.r, ctx));
		case 'ne':
			return !jsonEqual(value(e.l, ctx), value(e.r, ctx));
		case 'and':
			return evalExpr(e.l, ctx) && evalExpr(e.r, ctx);
		case 'or':
			return evalExpr(e.l, ctx) || evalExpr(e.r, ctx);
	}
}
