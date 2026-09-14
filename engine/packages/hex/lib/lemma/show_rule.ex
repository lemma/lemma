defmodule Lemma.ShowRule do
  @moduledoc """
  One local rule in `Lemma.Show.rules`.

  `type` is the raw `LemmaType` JSON map. `branches` is the authored default/unless
  table as a list of maps (`condition` optional on the default arm; `result` required).
  Expression trees stay raw maps tagged by `"type"` (same wire as the JSON API).
  `depends_on_rules` is the stored local planning topo list.
  """

  @type t :: %__MODULE__{
          type: map(),
          branches: [map()],
          depends_on_rules: [String.t()]
        }

  @enforce_keys [:type, :branches, :depends_on_rules]
  defstruct [:type, :branches, :depends_on_rules]

  @doc """
  Builds a `Lemma.ShowRule` from one value of the decoded `Lemma.show/4` JSON
  `"rules"` map.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      type: Map.fetch!(map, "type"),
      branches: Map.fetch!(map, "branches"),
      depends_on_rules: Map.fetch!(map, "depends_on_rules")
    }
  end
end
