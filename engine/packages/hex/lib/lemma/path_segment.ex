defmodule Lemma.PathSegment do
  @moduledoc """
  One `uses` hop on a Show data or rule path.

  `uses` is the import alias. `repository` is `nil` for the unnamed workspace;
  otherwise the interned repository name of the target.
  """

  @type t :: %__MODULE__{
          uses: String.t(),
          repository: String.t() | nil,
          spec: String.t()
        }

  @enforce_keys [:uses, :spec]
  defstruct [:uses, :repository, :spec]

  @doc """
  Builds a `Lemma.PathSegment` from one element of a Show `path` array.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      uses: Map.fetch!(map, "uses"),
      repository: Map.get(map, "repository"),
      spec: Map.fetch!(map, "spec")
    }
  end
end
