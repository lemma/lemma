defmodule Lemma.Show do
  @moduledoc """
  Typed projection of the map returned by `Lemma.show/4`: a Draft 2020-12 JSON
  Schema for the spec's evaluation-data inputs, with Lemma metadata under `x-lemma-*`.

  Validate evaluation data against `schema` (the decoded document, unchanged). The struct
  fields read the document: `spec` is `title`, `commentary` is `description`, `types`
  is `x-lemma-types` (local data declarations), `data` is `properties` (inputs rules
  need, as `Lemma.ShowData`), `required` the unbound ones, `rows` the instance-list
  row schemas in `$defs` (as `Lemma.ShowRow`), and the remaining fields the matching
  `x-lemma-*` keywords.

  Optional `repository` is the interned repository name (nil for the unnamed
  workspace). Rule values are `Lemma.ShowRule` structs (`type`, `path`, `branches`,
  `depends_on_rules`); each rule `type` is a raw `LemmaType` JSON map tagged by
  `"kind"`. `meta` and `types` values are raw JSON maps.

  `Lemma.show/4` itself returns the plain decoded map; call `from_map/1` on that map
  to get a typed struct.
  """

  alias Lemma.ShowData
  alias Lemma.ShowRow
  alias Lemma.ShowRule

  @type t :: %__MODULE__{
          repository: String.t() | nil,
          spec: String.t(),
          commentary: String.t() | nil,
          effective_from: String.t() | nil,
          effective_to: String.t() | nil,
          start_line: non_neg_integer(),
          source_type: String.t() | map() | nil,
          types: %{optional(String.t()) => map()},
          data: %{optional(String.t()) => ShowData.t()},
          required: [String.t()],
          rows: %{optional(String.t()) => ShowRow.t()},
          rules: %{optional(String.t()) => ShowRule.t()},
          meta: %{optional(String.t()) => map()},
          schema: map()
        }

  @enforce_keys [:spec, :start_line, :types, :data, :required, :rows, :rules, :meta, :schema]
  defstruct [
    :repository,
    :spec,
    :commentary,
    :effective_from,
    :effective_to,
    :start_line,
    :source_type,
    :types,
    :data,
    :required,
    :rows,
    :rules,
    :meta,
    :schema
  ]

  @doc """
  Builds a `Lemma.Show` struct from the map decoded from `Lemma.show/4`'s JSON.

  Raises `KeyError` if a required keyword (`title`, `properties`,
  `x-lemma-start-line`, `x-lemma-meta`, `x-lemma-types`, `x-lemma-rules`) is missing;
  that shape never comes from a real `Lemma.show/4` call, so a missing key here is a
  caller bug, not a recoverable `Show` variant.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      repository: Map.get(map, "x-lemma-repository"),
      spec: Map.fetch!(map, "title"),
      commentary: Map.get(map, "description"),
      effective_from: Map.get(map, "x-lemma-effective-from"),
      effective_to: Map.get(map, "x-lemma-effective-to"),
      start_line: Map.fetch!(map, "x-lemma-start-line"),
      source_type: Map.get(map, "x-lemma-source-type"),
      types: Map.fetch!(map, "x-lemma-types"),
      data:
        map
        |> Map.fetch!("properties")
        |> Map.new(fn {key, entry} -> {key, ShowData.from_map(entry)} end),
      required: Map.get(map, "required", []),
      rows:
        map
        |> Map.get("$defs", %{})
        |> Map.new(fn {name, row} -> {name, ShowRow.from_map(row)} end),
      rules:
        map
        |> Map.fetch!("x-lemma-rules")
        |> Map.new(fn {key, entry} -> {key, ShowRule.from_map(entry)} end),
      meta: Map.fetch!(map, "x-lemma-meta"),
      schema: map
    }
  end

  @doc "Local data declarations (`x-lemma-types`)."
  @spec types(t()) :: %{optional(String.t()) => map()}
  def types(%__MODULE__{types: types}), do: types
end
