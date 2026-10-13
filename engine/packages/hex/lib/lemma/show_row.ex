defmodule Lemma.ShowRow do
  @moduledoc """
  Row object schema of an instance list, as returned in `Lemma.Show.rows`: one
  `$defs` entry of the Show document per row spec, referenced from a list property's
  `items` `$ref`.

  `data` holds every row-spec input keyed relative to the row spec, dotted `uses`
  fields included. `required` lists the row fields a row rule needs that have no
  committed value.
  """

  alias Lemma.ShowData

  @type t :: %__MODULE__{
          title: String.t(),
          data: %{optional(String.t()) => ShowData.t()},
          required: [String.t()]
        }

  @enforce_keys [:title, :data, :required]
  defstruct [:title, :data, :required]

  @doc """
  Builds a `Lemma.ShowRow` from one value of the decoded Show document's `"$defs"`.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      title: Map.fetch!(map, "title"),
      data:
        map
        |> Map.fetch!("properties")
        |> Map.new(fn {key, entry} -> {key, ShowData.from_map(entry)} end),
      required: Map.get(map, "required", [])
    }
  end
end
