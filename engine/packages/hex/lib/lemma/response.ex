defmodule Lemma.Response do
  @moduledoc """
  Typed projection of the JSON map returned by `Lemma.evaluate/3`: evaluated rule
  results and the resolved temporal window of the spec version actually run.

  `rows` holds the list rows the results reach, in first-reached order, each a raw
  map with `"spec"`, `"effective"`, `"data"` and `"rules"`. A row reference's
  `"index"` points into it. Absent from the JSON when no result reaches a row.

  `Lemma.evaluate/3` itself still returns the plain decoded JSON map for backward
  compatibility; call `from_map/1` on that map to get a typed struct.
  """

  alias Lemma.RuleResult

  @type t :: %__MODULE__{
          spec: String.t(),
          effective: String.t(),
          spec_effective_from: String.t() | nil,
          spec_effective_to: String.t() | nil,
          results: %{optional(String.t()) => RuleResult.t()},
          rows: [map()]
        }

  @enforce_keys [:spec, :effective, :results]
  defstruct [
    :spec,
    :effective,
    :spec_effective_from,
    :spec_effective_to,
    results: %{},
    rows: []
  ]

  @doc """
  Builds a `Lemma.Response` from the map decoded from `Lemma.evaluate/3`'s JSON.

  Raises `KeyError` if a required field (`spec`, `effective`, `results`) is
  missing — that shape never comes from a real `Lemma.evaluate/3` call.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      spec: Map.fetch!(map, "spec"),
      effective: Map.fetch!(map, "effective"),
      spec_effective_from: Map.get(map, "spec_effective_from"),
      spec_effective_to: Map.get(map, "spec_effective_to"),
      results:
        map
        |> Map.fetch!("results")
        |> Map.new(fn {key, result} -> {key, RuleResult.from_map(result)} end),
      rows: Map.get(map, "rows", [])
    }
  end
end
