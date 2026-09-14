import type { Engine } from './lemma.bindings.js';
export { Engine, initSync } from './lemma.bindings.js';
export declare function init(): Promise<void>;
export declare function Lemma(): Promise<Engine>;

/** Resolved shape of {@link Engine.install}. */
export interface RepositoryInstallResult {
  source: string;
  id: string;
}

declare module './lemma.bindings.js' {
  namespace Engine {
    /** Create engine with named limit overrides. Unknown keys throw. */
    function withLimits(limits: Record<string, number>): Engine;
    /** Restore an engine from {@link Engine.snapshot} bytes. */
    function fromSnapshot(bytes: Uint8Array): Engine;
  }

  interface Engine {
    /**
     * Load Lemma source(s).
     * - string → volatile source in the default repository
     * - object → labeled sources (key insertion order); `[label, code][]` → labeled sources (array order)
     * Throws `EngineError[]` on failure. `null`/`undefined` rejected.
     */
    load(code: string): void;
    load(sources: Record<string, string> | Array<[string, string]>): void;

    /**
     * Download Lemma source from LemmaBase for `name` (e.g. `@org/pkg`). Resolves with
     * `{ source, id }`; does not load the engine and does not write `lemma_deps/`.
     * Rejects with `EngineError[]` like `load`.
     */
    install(name: string): Promise<RepositoryInstallResult>;

    /**
     * JSON serialization of `Vec<ResolvedRepository>` from [`Engine::list`]:
     * each item has `repository` (name, or omitted for the default repository) and `specs`
     * (`ListedSpec` rows: name, effective_from, effective_to).
     */
    list(): ResolvedRepository[];

    /**
     * Spec interface and temporal window at `effective`. Lemma text is {@link Engine.source}.
     */
    show(
      repository: string | null | undefined,
      spec: string,
      effective?: string | null,
    ): Show;

    /**
     * Formatted canonical Lemma source. Omit `spec` for whole-repository text.
     */
    source(
      repository: string | null | undefined,
      spec?: string | null,
      effective?: string | null,
    ): string;

    /**
     * Remove a temporal spec slice. `effective`: ISO datetime or omit for now.
     */
    remove(
      repository: string | null | undefined,
      spec: string,
      effective?: string | null,
    ): void;

    /**
     * Replace identities in `code` (atomic upsert; Path/Dependency prune siblings).
     * `attribute` is the source label (path or `@owner/repo`); omit for volatile.
     */
    update(
      repository: string | null | undefined,
      code: string,
      attribute?: string | null,
    ): void;

    /** Resource limits configured for this engine. */
    limits(): ResourceLimits;

    /**
     * Persist parsed specs + plans + limits as opaque bytes.
     * Restore with {@link Engine.fromSnapshot}.
     */
    snapshot(): Uint8Array;

    /**
     * Canonical formatting of Lemma source. Throws `EngineError` on parse error.
     * `attribute` is an optional path label used in error messages.
     */
    format(code: string, attribute?: string | null): string;

    /**
     * Structural quality recommendations across loaded specs. Advisory only.
     */
    quality(): Recommendation[];

    /**
     * Evaluate a spec. Pass integers as numbers, decimals as strings in `data`.
     */
    run(options: RunOptions): Response;
  }
}

/** Options for {@link Engine.run}. */
export interface RunOptions {
  /** Spec name (required). */
  spec: string;
  /** Repository qualifier (e.g. `@org/repo`), or omit for the default repository. */
  repository?: string | null;
  /** ISO datetime for temporal resolution, or omit for now. */
  effective?: string | null;
  /** Input data values. Pass integers as numbers, decimals as strings. */
  data?: Record<string, unknown>;
  /** Rule names to evaluate, or omit for all rules. */
  rules?: string[] | string | null;
  /** Include explanation tree in response. */
  explain?: boolean;
}

/**
 * Source location attached to an {@link EngineError}. Line and column are
 * 1-based; `length` is the UTF-8 byte length of the offending span.
 */
export interface EngineErrorSource {
  attribute: string;
  line: number;
  column: number;
  length: number;
}

/**
 * Structured error thrown by {@link Engine.run}, {@link Engine.show},
 * {@link Engine.load}, and {@link Engine.install}
 * (as an array), and rejected from {@link Engine.install} (as an array).
 *
 * - `kind` classifies the failure ("parsing" for syntax, "validation" for
 *   semantic/planning including bad data values, "missing_repository" when a
 *   referenced repo is not loaded, "request" for bad API input, etc.).
 * - `message` is the inner reason only. Callers that previously parsed
 *   `"Failed to parse data 'X' as Y: ..."` strings should now use `related_data`
 *   for attribution and `message` for the reason.
 * - `related_data` is non-null when the error is attributable to a specific data
 *   input declared by the spec (e.g. a field-level form validation failure).
 * - `source` points at the offending range in the original Lemma source.
 */
export interface EngineError {
  kind:
    | "parsing"
    | "validation"
    | "inversion"
    | "registry"
    | "missing_repository"
    | "request"
    | "resource_limit";
  message: string;
  related_data: string | null;
  spec: string | null;
  related_spec: string | null;
  source: EngineErrorSource | null;
  suggestion: string | null;
  /** Present for `missing_repository` and `registry` errors (`@…` id). */
  repository: string | null;
  /** Present only for `kind: "registry"`. */
  registry_kind:
    | "not_found"
    | "unauthorized"
    | "network_error"
    | "server_error"
    | "other"
    | null;
  /** Present only for `kind: "request"`. */
  request_kind: "spec_not_found" | "rule_not_found" | "invalid_request" | null;
  /** Present only for `kind: "resource_limit"`. */
  limit_name: string | null;
  limit_value: string | null;
  actual_value: string | null;
}

// ---------------------------------------------------------------------------
// Show envelope (return shape of Engine.show)
// ---------------------------------------------------------------------------

/**
 * API value fields shared by `RuleResult` (flattened into its top-level fields),
 * `ShowData.fill`, `ShowData.suggestion`, and range endpoints.
 * A `None` field is absent (not `null`) per Rust `skip_serializing_if`.
 * When present: always `display`, plus exactly one typed field.
 */
export interface RuleResultValueEndpoint {
  /** Engine-rendered string (`LiteralValue::display_value`). */
  display?: string;
  /** All declared measure units, keyed by unit name. */
  measure?: Record<string, string>;
  /** All declared ratio units, keyed by unit name. */
  ratio?: Record<string, string>;
  number?: string;
  boolean?: boolean;
  text?: string;
  date?: string;
  time?: string;
  calendar?: { value: string; unit: string };
}

/**
 * API value shared by `RuleResult` (flattened into its top-level fields),
 * `ShowData.fill`, and `ShowData.suggestion`. When present: always `display`,
 * plus exactly one typed field for a non-range value; `range` is set instead for a
 * range value. A range endpoint (`range.from`/`range.to`) never itself carries a
 * `range` field.
 */
export interface RuleResultValue extends RuleResultValueEndpoint {
  range?: { from: RuleResultValueEndpoint; to: RuleResultValueEndpoint };
}

/** Where a custom type's extension chain is rooted: local to this spec, or imported. */
export type TypeDefiningSpec = { kind: "local" } | { kind: "import" };

export type TypeExtends =
  | { kind: "primitive" }
  | {
      kind: "custom";
      parent: string;
      family: string;
      defining_spec: TypeDefiningSpec;
    };

/** A unit-scoped bound (Measure/DateRange/TimeRange/MeasureRange minimum/maximum/lower/upper). */
export interface NamedBound {
  value: string;
  unit: string;
}

export interface MeasureUnit {
  name: string;
  factor: { numer: string; denom: string };
  /** (measure_ref, exponent) pairs from a compound unit declaration (e.g. meter/second). */
  derived_measure_factors: [string, number][];
  decomposition: Record<string, number>;
  minimum?: string;
  maximum?: string;
  suggestion?: string;
}

export interface RatioUnit {
  name: string;
  value: { numer: string; denom: string };
  minimum?: string;
  maximum?: string;
  suggestion?: string;
}

/**
 * Discriminated union over the 12 Lemma type kinds reachable at the API boundary.
 * Field `kind` is the serde tag; kind-specific fields sit at the top level next to
 * `kind`, `name`, and `extends`. The `veto`/`undetermined` `TypeSpecification`
 * variants are internal sentinels that never reach a successfully planned API
 * response and are intentionally excluded.
 */
export type LemmaType =
  & { name: string | null; extends: TypeExtends }
  & (
    | { kind: "boolean"; help: string }
    | {
        kind: "measure";
        minimum: NamedBound | null;
        maximum: NamedBound | null;
        decimals: number | null;
        units: MeasureUnit[];
        traits: ("duration" | "calendar")[];
        decomposition: Record<string, number> | null;
        help: string;
      }
    | {
        kind: "number";
        minimum: string | null;
        maximum: string | null;
        decimals: number | null;
        help: string;
      }
    | {
        kind: "numberrange";
        lower: string | null;
        upper: string | null;
        minimum: string | null;
        maximum: string | null;
        help: string;
      }
    | {
        kind: "ratio";
        minimum: string | null;
        maximum: string | null;
        decimals: number | null;
        units: RatioUnit[];
        help: string;
      }
    | {
        kind: "ratiorange";
        lower: string | null;
        upper: string | null;
        minimum: string | null;
        maximum: string | null;
        units: RatioUnit[];
        help: string;
      }
    | {
        kind: "text";
        length: number | null;
        options: string[];
        help: string;
      }
    | { kind: "date"; minimum: string | null; maximum: string | null; help: string }
    | {
        kind: "daterange";
        lower: string | null;
        upper: string | null;
        minimum: NamedBound | null;
        maximum: NamedBound | null;
        help: string;
      }
    | { kind: "time"; minimum: string | null; maximum: string | null; help: string }
    | {
        kind: "timerange";
        lower: string | null;
        upper: string | null;
        minimum: NamedBound | null;
        maximum: NamedBound | null;
        help: string;
      }
    | {
        kind: "measurerange";
        lower: NamedBound | null;
        upper: NamedBound | null;
        minimum: NamedBound | null;
        maximum: NamedBound | null;
        units: MeasureUnit[];
        decomposition: Record<string, number> | null;
        help: string;
      }
  );

/** One input declared in a spec. Omitted fields are absent (not `null`). */
export interface ShowData {
  type: LemmaType;
  /** Spec literal or literal `with` binding; UIs may skip review. */
  fill?: RuleResultValue;
  /** `-> suggest ...` suggestion; prompt with prefill in interactive UIs. */
  suggestion?: RuleResultValue;
  /** Local rule names that transitively need this data after normalize. Empty = reuse catalog only. */
  needed_by_rules: string[];
}

/** Return shape of {@link Engine.run}. */
export interface Response {
  spec: string;
  effective: string;
  /** Declared temporal window of the resolved spec version actually evaluated. */
  spec_effective_from?: string;
  spec_effective_to?: string;
  results: Record<string, RuleResult>;
}

/**
 * A rule's result. Fields of `RuleResultValue` are flattened directly onto this
 * object (the Rust side uses `#[serde(flatten)]`), so a measure result's map appears
 * at `result.measure`, not nested under a `value` key.
 */
export type RuleResult = RuleResultValue & {
  vetoed: boolean;
  veto_reason?: string;
  rule_type: string;
  /** Input keys still unbound for this rule (run-data-aware; subset of Show.data with non-empty needed_by_rules). */
  missing_data?: string[];
  /** Present when `run(..., explain: true)`. Shape: engine/schemas/api.v1.json (`RuleResult.explanation`). */
  explanation?: Explanation;
};

/** One evaluated unless condition, stated as a fact. */
export interface Cause {
  condition: string;
  value: string;
  children?: ExplanationNode[];
}

export interface ConversionStep {
  role: "outcome" | "rule" | "source";
  text: string;
}

/** Nested explanation tree node (tagged by `type`). */
export type ExplanationNode =
  | {
      type: "rule";
      name: string;
      result: string;
      body: string;
      causes?: Cause[];
      children?: ExplanationNode[];
    }
  | {
      type: "compose";
      expression: string;
      operands: ExplanationNode[];
    }
  | {
      type: "data";
      name: string;
      display: string;
    }
  | {
      type: "data_unused";
      name: string;
    }
  | {
      type: "conversion";
      expression: string;
      steps: ConversionStep[];
      operands: ExplanationNode[];
    }
  | {
      type: "veto";
      message?: string;
    };

/** Root and nested rule explanation (same shape). */
export interface Explanation {
  type: "rule";
  name: string;
  result: string;
  body: string;
  causes?: Cause[];
  children?: ExplanationNode[];
}

/** Half-open `[effective_from, effective_to)` for one loaded temporal row. */
export interface ShowVersion {
  effective_from?: string;
  effective_to?: string;
}

/** Provenance of a loaded source. Externally tagged; the unit `Volatile` variant
 *  is the bare string `"volatile"`. */
export type SourceType = "volatile" | { path: string } | { dependency: string };

/** Parsed literal value. Externally tagged. */
export type LiteralValue =
  | { number: string }
  | { number_with_unit: [string, string] }
  | { text: string }
  | { date: string }
  | { time: string }
  | { boolean: "true" | "false" | "yes" | "no" }
  | { range: [LiteralValue, LiteralValue] };

/** Conversion target on a Show `as` expression. */
export type ShowConversionTarget =
  | {
      type:
        | "boolean"
        | "measure"
        | "measure_range"
        | "number"
        | "number_range"
        | "ratio"
        | "ratio_range"
        | "text"
        | "date"
        | "date_range"
        | "time"
        | "time_range";
    }
  | { unit: { unit_name: string } };

/** Resolved expression on a Show rule branch (tagged by `type`). */
export type ShowExpression =
  | ({ type: "literal" } & RuleResultValue)
  | { type: "data"; name: string }
  | { type: "rule"; name: string }
  | { type: "and"; left: ShowExpression; right: ShowExpression }
  | { type: "not"; operand: ShowExpression }
  | {
      type: "arithmetic";
      op: "add" | "subtract" | "multiply" | "divide" | "modulo" | "power";
      left: ShowExpression;
      right: ShowExpression;
    }
  | {
      type: "comparison";
      op:
        | "greater_than"
        | "less_than"
        | "greater_than_or_equal"
        | "less_than_or_equal"
        | "is"
        | "is_not";
      left: ShowExpression;
      right: ShowExpression;
    }
  | {
      type: "unit_conversion";
      operand: ShowExpression;
      target: ShowConversionTarget;
    }
  | {
      type: "math";
      op:
        | "sqrt"
        | "sin"
        | "cos"
        | "tan"
        | "asin"
        | "acos"
        | "atan"
        | "log"
        | "exp"
        | "abs"
        | "floor"
        | "ceil"
        | "round";
      operand: ShowExpression;
    }
  | { type: "veto"; message?: string }
  | { type: "now" }
  | {
      type: "date_relative";
      kind: "in_past" | "in_future";
      operand: ShowExpression;
    }
  | {
      type: "date_calendar";
      kind: "current" | "past" | "future" | "not_in";
      unit: "year" | "month" | "week";
      operand: ShowExpression;
    }
  | { type: "range_literal"; from: ShowExpression; to: ShowExpression }
  | {
      type: "past_future_range";
      kind: "in_past" | "in_future";
      operand: ShowExpression;
    }
  | {
      type: "range_containment";
      value: ShowExpression;
      range: ShowExpression;
    }
  | { type: "is_veto"; operand: ShowExpression };

/** One arm of a rule's flat last-match table. Default arm omits condition. */
export interface ShowBranch {
  condition?: ShowExpression;
  result: ShowExpression;
}

/** Local rule on Show: result type, branches, stored depends_on_rules. */
export interface ShowRule {
  type: LemmaType;
  branches: ShowBranch[];
  depends_on_rules: string[];
}

/** Return shape of {@link Engine.show}. */
export interface Show {
  spec: string;
  commentary?: string;
  effective_from?: string;
  effective_to?: string;
  start_line: number;
  source_type?: SourceType;
  versions?: ShowVersion[];
  data: Record<string, ShowData>;
  /** Local rule graph; measure/ratio units live under `type.units`. */
  rules: Record<string, ShowRule>;
  meta: Record<string, LiteralValue>;
}

/** Slim listed spec row (engine `list`). */
export interface ListedSpec {
  name: string;
  effective_from?: string;
  effective_to?: string;
}

/** Rust `ResolvedRepository` (engine `list`). */
export interface ResolvedRepository {
  /** Absent for the default unnamed repository (only named repositories carry a name). */
  repository?: string;
  specs: ListedSpec[];
}

/** Rust `ResourceLimits`. */
export interface ResourceLimits {
  max_source_size_bytes: number;
  max_expression_depth: number;
  max_expression_count: number;
  max_data_value_bytes: number;
  max_loaded_bytes: number;
  max_sources: number;
  max_normalized_expression_nodes: number;
  max_spec_dependency_depth: number;
  max_dag_specs: number;
  max_normal_form_depth: number;
}

/** Source location on a quality recommendation. Same shape as {@link EngineErrorSource}. */
export interface RecommendationSource {
  attribute: string;
  line: number;
  column: number;
  length: number;
}

/** One structural quality recommendation from {@link Engine.quality}. */
export interface Recommendation {
  /** Advisory prose only. Temporal identity is in `spec` + `effective_from`. */
  message: string;
  spec: string;
  /** Declared temporal start of the analyzed slice, or null for Origin. */
  effective_from: string | null;
  repository: string | null;
  source: RecommendationSource;
}
