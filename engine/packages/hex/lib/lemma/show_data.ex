defmodule Lemma.ShowData do
  @moduledoc """
  One declared data slot in a spec, as returned in `Lemma.Show.data`.

  Empty `needed_by_rules` means offered for reuse (`data x: alias.slot`), not an
  eval intake key for this spec.

  `path` is the import hops to this slot; empty means this spec.

  `type` is the raw `LemmaType` JSON map, a Rust discriminated union tagged by its
  `"kind"` string. It is intentionally left untyped here: pattern-match on it
  directly (e.g. `%{"kind" => "measure", "units" => units} = entry.type`) rather than
  routing through a parallel Elixir struct hierarchy for 12 type kinds.
  """

  alias Lemma.PathSegment

  @type t :: %__MODULE__{
          type: map(),
          path: [PathSegment.t()],
          fill: map() | nil,
          suggestion: map() | nil,
          needed_by_rules: [String.t()]
        }

  @enforce_keys [:type, :path]
  defstruct type: nil, path: [], fill: nil, suggestion: nil, needed_by_rules: []

  @doc """
  Builds a `Lemma.ShowData` from one value of the decoded `Lemma.show/4` JSON
  `"data"` map. `fill`/`suggestion` are absent (not `null`) from the API JSON
  when unset; `Map.get/3` yields `nil` for both. `path` is required (empty list
  is valid).
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      type: Map.fetch!(map, "type"),
      path: map |> Map.fetch!("path") |> Enum.map(&PathSegment.from_map/1),
      fill: Map.get(map, "fill"),
      suggestion: Map.get(map, "suggestion"),
      needed_by_rules: Map.get(map, "needed_by_rules", [])
    }
  end
end
