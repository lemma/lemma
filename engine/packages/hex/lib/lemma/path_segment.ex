defmodule Lemma.PathSegment do
  @moduledoc """
  One hop on a Show data or rule path: a `uses` alias, or (with `row`) a row
  written in the data list named `uses`.

  `uses` is the import alias, or the data list name when `row` is set. `row` is
  the 1-based position of the row written in that list; `nil` for a `uses` hop.
  `repository` is `nil` for the unnamed workspace; otherwise the interned
  repository name of the target.
  """

  @type t :: %__MODULE__{
          uses: String.t(),
          row: pos_integer() | nil,
          repository: String.t() | nil,
          spec: String.t()
        }

  @enforce_keys [:uses, :spec]
  defstruct [:uses, :row, :repository, :spec]

  @doc """
  Builds a `Lemma.PathSegment` from one element of a Show `path` array.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      uses: Map.fetch!(map, "uses"),
      row: Map.get(map, "row"),
      repository: Map.get(map, "repository"),
      spec: Map.fetch!(map, "spec")
    }
  end
end
