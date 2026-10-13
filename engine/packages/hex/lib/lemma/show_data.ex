defmodule Lemma.ShowData do
  @moduledoc """
  One evaluation-data input of a spec, as returned in `Lemma.Show.data`: one `properties`
  entry of the Show JSON Schema document, keyed by its input key.

  Empty `needed_by_rules` (`x-lemma-needed-by-rules`) means offered for reuse
  (`data x: alias.slot`), not an eval intake key for this spec.

  `path` (`x-lemma-path`) is the import hops to this slot; empty means this spec.

  `type` is the raw `x-lemma-type` map (`ShowPropertyType`): a LemmaType without
  `help` and without text `options`/`length`. Those live on the property schema as
  `description`, `enum` and `minLength`/`maxLength` (`schema`). Pattern-match on
  `type` by `"kind"` (e.g. `%{"kind" => "measure", "units" => units} = entry.type`).
  A `"list"` type carries `"element"` (a bare type specification; `"kind" => "spec"`
  for instance lists) and `"key"` (`"numbered"` or `%{"fields" => [...]}`).

  `value` (`x-lemma-value`) of an instance list lists its source rows in order: each
  item of `"list"` has only `"fields"`, the fields that row sets, keyed relative to
  the row spec; neither the list nor its rows carry `"result"`. A literal field is a
  typed `RuleResultValue`; a referenced field carries only `"reference"` (the input
  key of the referenced data). A list has no `value` when it has no source rows.

  `schema` is the raw property schema map, for validating evaluation data.
  """

  alias Lemma.PathSegment

  @type t :: %__MODULE__{
          type: map(),
          path: [PathSegment.t()],
          value: map() | nil,
          suggestion: map() | nil,
          needed_by_rules: [String.t()],
          schema: map()
        }

  @enforce_keys [:type, :path, :needed_by_rules, :schema]
  defstruct [:type, :path, :value, :suggestion, :needed_by_rules, :schema]

  @doc """
  Builds a `Lemma.ShowData` from one value of the decoded Show document's
  `"properties"` (or a row schema's `"properties"`). `x-lemma-value` and
  `x-lemma-suggestion` are absent when unset; both yield `nil`.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      type: Map.fetch!(map, "x-lemma-type"),
      path: map |> Map.fetch!("x-lemma-path") |> Enum.map(&PathSegment.from_map/1),
      value: Map.get(map, "x-lemma-value"),
      suggestion: Map.get(map, "x-lemma-suggestion"),
      needed_by_rules: Map.fetch!(map, "x-lemma-needed-by-rules"),
      schema: map
    }
  end
end
