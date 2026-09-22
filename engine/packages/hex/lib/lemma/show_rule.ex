defmodule Lemma.ShowRule do
  @moduledoc """
  One rule in `Lemma.Show.rules` (this spec's graph: local plus reachable imports).

  `type` is the raw `LemmaType` JSON map. `path` is the import hops to this rule;
  empty means this spec. `branches` is the authored default/unless table as a list
  of maps (`condition` optional on the default arm; `result` required).
  Expression trees stay raw maps tagged by `"type"` (same wire as the JSON API).
  `depends_on_rules` is the stored planning topo list as `input_key` strings.
  """

  alias Lemma.PathSegment

  @type t :: %__MODULE__{
          type: map(),
          path: [PathSegment.t()],
          branches: [map()],
          depends_on_rules: [String.t()]
        }

  @enforce_keys [:type, :path, :branches, :depends_on_rules]
  defstruct [:type, :path, :branches, :depends_on_rules]

  @doc """
  Builds a `Lemma.ShowRule` from one value of the decoded `Lemma.show/4` JSON
  `"rules"` map. `path` is required (empty list is valid).
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      type: Map.fetch!(map, "type"),
      path: map |> Map.fetch!("path") |> Enum.map(&PathSegment.from_map/1),
      branches: Map.fetch!(map, "branches"),
      depends_on_rules: Map.fetch!(map, "depends_on_rules")
    }
  end
end
