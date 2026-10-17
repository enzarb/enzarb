## Summary

Replace the hand-written parsing in `ui/src/lib/server/config.ts` with one valibot schema. The schema covers required keys, optional keys and defaults. `Config` becomes `v.InferOutput<typeof ConfigSchema>`, and the `RequiredKey = Exclude<…>` list and the `REQUIRED` map are removed. Only `config.ts` and `config.spec.ts` change. valibot is already a dependency, and `projects.ts` and `onboarding.ts` already import it as `import * as v from 'valibot'`.

## Approach

**Schema shape.** The input is the env map (`Record<string, string | undefined>`), so the schema is `v.pipe(v.object({...env var names...}), v.transform(...))`. Its output is the camelCase `Config`. The keys of the input object are the real env var names, so issue paths map directly to the names the error messages need.

**Per-field helpers.**
- A `trimmed` step: `v.pipe(v.optional(v.string()), v.transform(s => s?.trim() || undefined))`. Blank counts as unset, as today.
- `required(name)`: `trimmed` followed by a check that the value is defined. A missing value fails with an internal marker message.
- Optional fields use `v.optional`, or a transform that supplies the default.

**Preserving the exact error messages.** The priority is that behaviour and messages stay unchanged.
- Each field gets its current message, e.g. `v.check(isPositiveInt, 'ENZARB_STATS_RETENTION must be a positive whole number of days')`.
- `parseConfig` calls `v.safeParse`. On failure it builds the error from the issues:
  - If any issue is a "missing required" issue, it throws `Missing required environment variables: A, B, …`. The names come from `issue.path[0].key`, in schema key order, which is the same as today's order.
  - Otherwise it throws the first issue's message.
- This keeps today's precedence: the missing-key check runs before any format check, and format errors are reported one at a time. A test pins that precedence.

**Cross-field logic.** These are done in the final `v.transform` or `v.rawTransform`. `rawTransform` can add issues, so it is used where errors are possible.
- `publicUrl` is validated and normalised to `url.origin` inside its own field pipe (absolute URL, then http(s) protocol, via a transform that uses `URL`).
- VAPID needs the normalised `publicUrl` for the subject default. It is a `rawTransform` on the whole object that:
  - returns `undefined` when neither key is set;
  - adds an issue when only one key is set;
  - adds an issue when a key is not base64url;
  - adds an issue when the subject is invalid.
  - The messages are the same as now.
- GitHub App id and key are validated together with the existing regexes, after the `\n` expansion. Both fields are set only if both are valid. A malformed value is silently dropped and the value never appears in a message. This is done with a lenient field schema that yields `undefined` on mismatch (`v.fallback` or a transform that returns `undefined`), so it never produces an issue.
- `ENZARB_COMPONENT_VERSIONS` is a field pipe: `JSON.parse` in a transform, then `v.object({chart: v.optional(v.string('…chart must be a string')), components: ...})`. The messages `must be valid JSON`, `must be a JSON object`, `.components must be an object` and `.components.<key> must be a string` are reproduced. The per-key message uses `v.record` with a custom message, or an explicit check that includes the key.
  - If valibot's object and record issue shapes make the exact messages awkward, the fallback is a single `v.rawTransform` that reuses the current `parseComponentVersions` body. The pipeline stays valibot, the messages stay exact, and the spec is untouched. I would try the declarative version first.
- Other fields:
  - `ENZARB_REALTIME_LEGACY_TOKEN_URL` is lowercased, checked against `true|false`, and then mapped to boolean. Unset gives `false`.
  - `NEEDS_YOU_STALE_HOURS` defaults to 24, must be finite and greater than 0, and keeps the deprecation comment.
  - `ENZARB_STATS_RETENTION` defaults to 90 and must be a positive integer.
  - `ENZARB_GITHUB_APP_SLUG` defaults to `DEFAULT_GITHUB_APP_SLUG` and must match the existing regex.

**Types.** `Config = v.InferOutput<typeof ConfigSchema>`. `VapidConfig` and `ComponentVersions` are also inferred from their sub-schemas and keep the same exported names. `githubAppId?`, `githubAppPrivateKey?` and `vapid?` stay optional, so no downstream type changes. The per-field JSDoc moves onto the schema entries or the output object definition. I will make sure the inferred type has the same keys as the interface it replaces, for example with a temporary type-level assert while developing.

**Rejected alternatives.**
- Using valibot's default messages. That would change the operator-visible errors and break "behaviour unchanged".
- Several independent `v.safeParse` calls. That does not give a single schema that produces `Config`.
- Keeping the `REQUIRED` map next to the schema. That would leave a second source of truth.

## Changes

- `ui/src/lib/server/config.ts`:
  - Add `ConfigSchema` and the per-field helper schemas.
  - Derive `Config`, `VapidConfig` and `ComponentVersions` from them.
  - Delete `RequiredKey`, `REQUIRED`, `parseVapid`, `parseComponentVersions` (or keep it inside a `rawTransform` per the fallback above) and the manual validation body of `parseConfig`.
  - `parseConfig(source)` becomes `safeParse` plus the issue-to-error mapping.
  - `getConfig`, `DEFAULT_GITHUB_APP_SLUG` and the module cache are unchanged.
- `ui/src/lib/server/config.spec.ts`: keep all existing cases and assertions, since they define the unchanged behaviour. Add the cases listed under Tests.
- Callers (`getConfig` importers and the `vi.mock`s in other specs) need no change. They only use the exported names and the `Config` shape.

## Tests

The existing 20-odd assertions in `config.spec.ts` should pass unmodified. That is the main evidence of unchanged behaviour. New cases:
- Missing keys are reported in schema order, and missing-key errors take precedence over a simultaneously invalid value.
- A leaked-value check: the error text for a bad `ENZARB_GITHUB_APP_PRIVATE_KEY` never occurs, and the value is dropped. Also: a valid id with no key leaves both unset.
- `ENZARB_REALTIME_LEGACY_TOKEN_URL` accepts `TRUE` and ` False `.
- Whitespace-only optional values fall back to their defaults (slug, retention, stale hours).
- VAPID subject default uses the normalised origin even when `ENZARB_PUBLIC_URL` has a path.
- A type-level check, e.g. `expectTypeOf<Config['githubAppId']>().toEqualTypeOf<string | undefined>()`.

Run in `ui/`: `npm run check && npm run lint && npm test`. I have not run these yet.

## Risks and open questions

- **Error precedence and "first error only" semantics.** The mapping code must replicate them exactly. This is the main review focus.
- **Component-versions messages.** Reproducing them declaratively may need custom message functions. The `rawTransform` fallback is described above.
- **Empty-string `ENZARB_PUBLIC_URL` and similar.** Today these are reported as missing, not as invalid. The trimmed-to-undefined step keeps that.
- **Type inference with optional fields.** `v.optional` with a transform to `undefined` can infer `key: T | undefined` rather than `key?: T`. If it does, I will build the object by spreading conditionally, as the current code does, and annotate the output type so the optional keys stay optional. Downstream types must not change.
- **valibot version.** `^1.5.0` is installed. I will check any API I use against it, since I have not run it here.
- **Assumption:** exact message preservation is preferred over idiomatic valibot messages.

## Out of scope

- Removing the deprecated `NEEDS_YOU_STALE_HOURS`.
- Removing the legacy realtime token flag.
- Changes to Helm values, docs, or any other component.
- Moving other modules to this config pattern.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
