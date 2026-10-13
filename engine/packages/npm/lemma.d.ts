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
    /** Semver of the loaded WASM build (e.g. `"0.9.12"`). */
    readonly version: string;

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
     * JSON serialization of `Vec<Repository>` from [`Engine::specs`]:
     * each item has `repository` (name, or omitted for the default repository) and `specs`
     * (`SpecVersion` rows: name, effective_from, effective_to, dependencies).
     */
    specs(): Repository[];

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
     * Evaluate a spec. Large integers: digit `string` or `bigint`. Safe integers may be numbers. Decimals: strings.
     */
    evaluate(options: EvaluateOptions): Response;
    /**
     * Deprecated alias of {@link Engine.evaluate}.
     * @deprecated Use {@link Engine.evaluate}
     */
    run(options: EvaluateOptions): Response;
  }
}

/** Overlay tree for {@link Engine.evaluate} `data`. JSON `number` / JS `number` are rejected. */
export type EvaluateDataValue =
  | string
  | boolean
  | bigint
  | { [unit: string]: EvaluateDataValue }
  | EvaluateDataValue[];

/**
 * @deprecated Use {@link EvaluateDataValue}
 */
export type RunDataValue = EvaluateDataValue;

/** Options for {@link Engine.evaluate}. */
export interface EvaluateOptions {
  /** Spec name (required). */
  spec: string;
  /** Repository qualifier (e.g. `@org/repo`), or omit for the default repository. */
  repository?: string | null;
  /** ISO datetime for temporal resolution, or omit for now. */
  effective?: string | null;
  /**
   * Input data values.
   * Magnitudes: digit `string` or `bigint` (exact). `number` is always a double and is rejected.
   * Booleans, single-unit maps (`{ eur: "84" }` or `{ eur: 84n }`), and arrays also accepted.
   */
  data?: Record<string, EvaluateDataValue>;
  /** Rule names to evaluate, or omit for all rules. */
  rules?: string[] | string | null;
  /** Include explanation tree in response. */
  explain?: boolean;
}

/**
 * @deprecated Use {@link EvaluateOptions}
 */
export type RunOptions = EvaluateOptions;

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
 * Structured error thrown by {@link Engine.evaluate}, {@link Engine.show},
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
 * `ShowProperty["x-lemma-value"]`, `ShowProperty["x-lemma-suggestion"]`, and range endpoints.
 * A `None` field is absent (not `null`) per Rust `skip_serializing_if`.
 * When present: always `result`, plus exactly one typed field.
 */
export interface RuleResultValueEndpoint {
  /** Engine-rendered one-liner (`LiteralValue::display_value`). */
  result?: string;
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
  /** Written unit for a measure or ratio value (including range endpoints). */
  unit?: string;
}

/**
 * API value shared by `RuleResult` (flattened into its top-level fields),
 * `ShowProperty["x-lemma-value"]`, and `ShowProperty["x-lemma-suggestion"]`. When present: always `result`,
 * plus exactly one typed field for a non-range value; `range` is set instead for a
 * range value. A range endpoint (`range.from`/`range.to`) never itself carries a
 * `range` field. An instance list in Show is the exception: it carries `list` only,
 * and each source row `fields` only.
 */
export interface SpecRuleResult {
  vetoed: boolean;
  veto_reason?: string;
  rule_type: string;
  result?: RuleResultValue;
}

/** Spec instance returned by a rule. `data` holds one evaluated entry per data slot, relative to the spec. */
export interface SpecResult {
  repository?: string;
  spec: string;
  effective: string;
  instance: string;
  /** One hop per `uses` from the evaluated spec to the instance. Never empty. */
  path: PathSegment[];
  data: Record<string, SpecRuleResult>;
  rules: Record<string, SpecRuleResult>;
}

/** A list row in a result: `index` points into {@link Response.rows}. */
export interface RowReference {
  repository?: string;
  spec: string;
  /** Resolved row effective date; empty for the origin row. */
  effective: string;
  /** Row path relative to its holder, e.g. `lines.2`. */
  instance: string;
  index: number;
}

/** One list row the results reach, filled once however many results reach it. */
export interface RowResult {
  repository?: string;
  spec: string;
  effective: string;
  data: Record<string, SpecRuleResult>;
  rules: Record<string, SpecRuleResult>;
}

export interface RuleResultValue extends RuleResultValueEndpoint {
  range?: { from: RuleResultValueEndpoint; to: RuleResultValueEndpoint };
  spec?: SpecResult;
  /** Set when the value is one list row. */
  row?: RowReference;
  /** Set when the value is a list: items in list order; rows carry `row`. */
  list?: RuleResultValue[];
  /**
   * Show only: on a source row of an instance list value, the fields the row sets,
   * keyed relative to the row spec. Such rows and their list have no `result`;
   * row order is array order.
   */
  fields?: Record<string, RuleResultValue>;
  /**
   * Show only: on a source-row field whose value is a reference; the input key of
   * the referenced data in the spec declaring the list.
   */
  reference?: string;
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
 * Discriminated union over the 14 Lemma type kinds reachable at the API boundary.
 * Field `kind` is the serde tag; kind-specific fields sit at the top level next to
 * `kind`, `name`, and `extends`. The `veto`/`undetermined` `TypeSpecification`
 * variants are internal sentinels that never reach a successfully planned API
 * response and are intentionally excluded.
 */
export type LemmaType = { name: string | null; extends: TypeExtends } & (
  | TypeSpecification
  | ListTypeSpecification
);

/** List type: rows of `element`. */
export interface ListTypeSpecification {
  kind: "list";
  /** Row type: a spec for instance lists. Never a list; lists do not nest. */
  element: TypeSpecification;
  key: ListKey;
}

/** How a list addresses its rows. */
export type ListKey = "numbered" | { fields: string[] };

/** Kind-specific fields of a non-list type, without `name` and `extends`; also a list element. */
export type TypeSpecification =
    | { kind: "boolean"; help: string }
    | {
        kind: "measure";
        minimum: NamedBound | null;
        maximum: NamedBound | null;
        decimals: number | null;
        units: MeasureUnit[];
        traits: ("duration" | "calendar")[];
        decomposition: Record<string, number> | null;
        /** Written unit when this type describes a value (omit on schema-only types). */
        unit?: string;
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
        kind: "number_range";
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
        /** Written unit when this type describes a value (omit on schema-only types). */
        unit?: string;
        help: string;
      }
    | {
        kind: "ratio_range";
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
        kind: "date_range";
        lower: string | null;
        upper: string | null;
        minimum: NamedBound | null;
        maximum: NamedBound | null;
        help: string;
      }
    | { kind: "time"; minimum: string | null; maximum: string | null; help: string }
    | {
        kind: "time_range";
        lower: string | null;
        upper: string | null;
        minimum: NamedBound | null;
        maximum: NamedBound | null;
        help: string;
      }
    | {
        kind: "measure_range";
        lower: NamedBound | null;
        upper: NamedBound | null;
        minimum: NamedBound | null;
        maximum: NamedBound | null;
        units: MeasureUnit[];
        decomposition: Record<string, number> | null;
        help: string;
      }
    | {
        kind: "spec";
        spec: string;
        effective: string;
        repository?: string;
      };

/** One uses hop on a Show data or rule path. */
export interface PathSegment {
  /** Uses alias for this hop. */
  uses: string;
  /** Target repository name; omitted for the unnamed workspace. */
  repository?: string;
  /** Resolved target spec name. */
  spec: string;
}

/** Drops the keywords a {@link ShowProperty} states in JSON Schema: `help` (`description`), text `options` (`enum`) and `length` (`minLength`/`maxLength`). */
type WithoutStandardKeywords<T> = T extends { kind: "text" }
  ? Omit<T, "help" | "options" | "length">
  : Omit<T, "help">;

/** `x-lemma-type` of a {@link ShowProperty}: the {@link LemmaType} without its standard-keyword fields. */
export type ShowPropertyType = { name: string | null; extends: TypeExtends } & (
  | WithoutStandardKeywords<TypeSpecification>
  | (Omit<ListTypeSpecification, "element"> & {
      element: WithoutStandardKeywords<TypeSpecification>;
    })
);

/** An evaluation-data JSON value: literal string, boolean, list, or object (list row, single-unit measure map). */
export type EvaluateDataJson =
  | string
  | boolean
  | EvaluateDataJson[]
  | { [key: string]: EvaluateDataJson };

/**
 * @deprecated Use {@link EvaluateDataJson}
 */
export type RunDataJson = EvaluateDataJson;

/** `items` of a list {@link ShowProperty}: element literal schema, or `$ref` to a row schema in `Show.$defs`. */
export type ShowItems =
  | {
      type?: "string" | "boolean";
      oneOf?: Record<string, unknown>[];
      pattern?: string;
      enum?: string[];
      minLength?: number;
      maxLength?: number;
    }
  | { $ref: string };

/** One evaluation-data input of a {@link Show} document, keyed by input key (dotted keys included). */
export interface ShowProperty {
  type?: "string" | "boolean" | "array";
  /** Alternatives: text, boolean, or a single-unit object for measure and ratio data. */
  oneOf?: Record<string, unknown>[];
  /** Literal grammar of number, measure and ratio inputs (ECMA-262). */
  pattern?: string;
  /** Text options. */
  enum?: string[];
  minLength?: number;
  maxLength?: number;
  /** Type help. */
  description?: string;
  /** Evaluation-data JSON of `x-lemma-value`. */
  default?: EvaluateDataJson;
  /** Evaluation-data JSON of `x-lemma-suggestion`. */
  examples?: [EvaluateDataJson];
  items?: ShowItems;
  /** `max_list_rows`. */
  maxItems?: number;
  "x-lemma-type": ShowPropertyType;
  /** Import hops to this slot. Empty means this spec. */
  "x-lemma-path": PathSegment[];
  /** `x-lemma-rules` keys that transitively need this data after normalize. Empty = reuse catalog only. */
  "x-lemma-needed-by-rules": string[];
  /** Spec literal or literal `with` binding; UIs may skip review. */
  "x-lemma-value"?: RuleResultValue;
  /** `-> suggest ...` suggestion; prompt with prefill in interactive UIs. */
  "x-lemma-suggestion"?: RuleResultValue;
}

/** Row object schema of an instance list, one per row spec in `Show.$defs`. */
export interface ShowRowSchema {
  title: string;
  type: "object";
  additionalProperties: true;
  properties: Record<string, ShowProperty>;
  required?: string[];
}

/** Return shape of {@link Engine.evaluate}. */
export interface Response {
  spec: string;
  effective: string;
  /** Declared temporal window of the resolved spec version actually evaluated. */
  spec_effective_from?: string;
  spec_effective_to?: string;
  results: Record<string, RuleResult>;
  /** List rows the results reach, in first-reached order. Omitted when none. */
  rows?: RowResult[];
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
  /** Input keys still unbound for this rule (evaluation-data-aware; subset of Show.properties with non-empty x-lemma-needed-by-rules). */
  missing_data?: string[];
  /** Present when `evaluate(..., explain: true)`. Shape: engine/schemas/api.v1.json (`RuleResult.explanation`). */
  explanation?: Explanation;
};

/** One evaluated unless condition, stated as a fact. */
export interface Cause {
  condition: string;
  value: string;
  children?: ExplanationNode[];
}

export interface ConversionStep {
  role: "outcome" | "rule";
  text: string;
}

/** One narrated rule body in a root explanation's `rules` table. */
export type ExplainedRule = {
  id: number;
  name: string;
  body: string;
  causes?: Cause[];
  children?: ExplanationNode[];
} & RuleResultValue;

/** Nested explanation tree node (tagged by `type`). */
export type ExplanationNode =
  | ({
      type: "rule_ref";
      id: number;
      name: string;
    } & RuleResultValue)
  | {
      type: "compose";
      expression: string;
      operator?: "add" | "subtract" | "multiply" | "divide" | "modulo" | "power";
      operands: ExplanationNode[];
    }
  | ({
      type: "data";
      name: string;
    } & RuleResultValue)
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
    }
  | {
      type: "row";
      number: number;
      appended?: boolean;
      key?: ExplanationNode[];
      fields: ExplanationNode[];
    }
  | {
      type: "filter";
      condition: string;
      matched: number;
      total: number;
      children?: ExplanationNode[];
    };

/** Root rule explanation under `RuleResult.explanation`. */
export type Explanation = {
  type: "rule";
  name: string;
  body: string;
  causes?: Cause[];
  children?: ExplanationNode[];
  /** Narrated rules reachable from this root; `rule_ref` nodes point here by `id`. */
  rules?: ExplainedRule[];
} & RuleResultValue;

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

/** One operator step of an `arithmetic` show node: `op operand`. */
export interface ShowArithmeticStep {
  op: "add" | "subtract" | "multiply" | "divide" | "modulo" | "power";
  operand: ShowExpression;
}

/** Resolved expression on a Show rule branch (tagged by `type`). */
export type ShowExpression =
  | ({ type: "literal" } & RuleResultValue)
  | { type: "data"; name: string }
  | { type: "rule"; name: string }
  | { type: "and"; operands: ShowExpression[] }
  | { type: "not"; operand: ShowExpression }
  | {
      type: "arithmetic";
      first: ShowExpression;
      steps: ShowArithmeticStep[];
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
      targets: ShowConversionTarget[];
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
  | {
      type: "range_bound";
      bound: "lower" | "upper";
      operand: ShowExpression;
    }
  | { type: "is_veto"; operand: ShowExpression }
  | { type: "spec_member"; base: string; member: string }
  /** Field of a computed row: `(lines -> first).price`. */
  | { type: "member"; base: ShowExpression; member: string }
  /** Field of the row a list command visits (`price` in `lines -> where price > 1`). */
  | { type: "row_field"; field: string }
  /** The item a value-list command visits (`>= 100 eur` in `fares -> where >= 100 eur`). */
  | { type: "row_value" }
  /** One `field: value` of a `with` row. */
  | { type: "row_field_value"; field: string; value: ShowExpression }
  | {
      type: "list_command";
      list: ShowExpression;
      command: ShowListCommand;
      /** Set only for `sort`. */
      direction?: "asc" | "desc";
      args: ShowExpression[];
    };

/** List command on a Show `list_command` expression. */
export type ShowListCommand =
  | "find"
  | "first"
  | "last"
  | "count"
  | "sum"
  | "avg"
  | "product"
  | "min"
  | "max"
  | "any"
  | "all"
  | "where"
  | "sort"
  | "top"
  | "with";

/** One arm of a rule's flat last-match table. Default arm omits condition. */
export interface ShowBranch {
  condition?: ShowExpression;
  result: ShowExpression;
}

/** Rule on Show: result type, path, branches, depends_on_rules. */
export interface ShowRule {
  type: LemmaType;
  /** Import hops to this rule. Empty means this spec. */
  path: PathSegment[];
  branches: ShowBranch[];
  depends_on_rules: string[];
}

/**
 * Return shape of {@link Engine.show}: a Draft 2020-12 JSON Schema for the spec's evaluation-data inputs.
 * Validate an evaluation-data payload against it as is; `x-lemma-*` keywords carry Lemma metadata.
 */
export interface Show {
  $schema: "https://json-schema.org/draft/2020-12/schema";
  /** Spec name. */
  title: string;
  /** Spec commentary. */
  description?: string;
  type: "object";
  additionalProperties: true;
  /** Inputs rules need (non-empty `x-lemma-needed-by-rules`). */
  properties: Record<string, ShowProperty>;
  /** Inputs a rule needs that have no committed value. */
  required?: string[];
  /** Instance-list row schemas, referenced from `items.$ref`. */
  $defs?: Record<string, ShowRowSchema>;
  /** Interned repository name; omitted for the unnamed workspace. */
  "x-lemma-repository"?: string;
  "x-lemma-effective-from"?: string;
  "x-lemma-effective-to"?: string;
  "x-lemma-start-line": number;
  "x-lemma-source-type"?: SourceType;
  "x-lemma-meta": Record<string, LiteralValue>;
  /** Local data declarations (empty path), declaration order. */
  "x-lemma-types": Record<string, LemmaType>;
  /** This spec's rule graph (local plus reachable imports); measure/ratio units under `type.units`. */
  "x-lemma-rules": Record<string, ShowRule>;
}

/** One `uses` target on a spec version (engine `specs`). */
export interface SpecDependency {
  repository?: string;
  name: string;
  effective?: string;
}

/** One temporal spec version (engine `specs`). */
export interface SpecVersion {
  name: string;
  effective_from?: string;
  effective_to?: string;
  dependencies?: SpecDependency[];
}

/** Rust `Repository` (engine `specs`). */
export interface Repository {
  /** Absent for the default unnamed repository (only named repositories carry a name). */
  repository?: string;
  specs: SpecVersion[];
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
  max_list_rows: number;
  max_list_depth: number;
  max_list_nodes: number;
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
