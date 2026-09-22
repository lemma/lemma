defmodule Lemma.Show do
  @moduledoc """
  Typed projection of the JSON map returned by `Lemma.show/4`: spec interface and
  resolved temporal window.

  Optional `repository` is the interned repository name (nil for the unnamed
  workspace). Each `data` entry's `type` and each `rules` entry's `type` are raw
  `LemmaType` JSON maps (a Rust discriminated union tagged by `"kind"`). Rule values
  are `Lemma.ShowRule` structs (`type`, `path`, `branches`, `depends_on_rules`).
  `meta` values are raw `LiteralValue` JSON maps. Pattern-match on those tags
  directly rather than through a parallel Elixir struct hierarchy — see
  `Lemma.ShowData` and `Lemma.ShowRule`.

  `Lemma.show/4` itself still returns the plain decoded JSON map for backward
  compatibility; call `from_map/1` on that map to get a typed struct.
  """

  alias Lemma.ShowData
  alias Lemma.ShowRule
  alias Lemma.ShowVersion

  @type t :: %__MODULE__{
          repository: String.t() | nil,
          spec: String.t(),
          commentary: String.t() | nil,
          effective_from: String.t() | nil,
          effective_to: String.t() | nil,
          start_line: non_neg_integer(),
          source_type: String.t() | map() | nil,
          versions: [ShowVersion.t()],
          data: %{optional(String.t()) => ShowData.t()},
          rules: %{optional(String.t()) => ShowRule.t()},
          meta: %{optional(String.t()) => map()}
        }

  @enforce_keys [:spec, :start_line]
  defstruct [
    :repository,
    :spec,
    :commentary,
    :effective_from,
    :effective_to,
    :start_line,
    :source_type,
    versions: [],
    data: %{},
    rules: %{},
    meta: %{}
  ]

  @doc """
  Builds a `Lemma.Show` struct from the map decoded from `Lemma.show/4`'s JSON.

  Raises `KeyError` if a required field (`spec`, `start_line`, `data`, `rules`) is
  missing — that shape never comes from a real `Lemma.show/4` call, so a missing
  key here is a caller bug, not a recoverable `Show` variant.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      repository: Map.get(map, "repository"),
      spec: Map.fetch!(map, "spec"),
      commentary: Map.get(map, "commentary"),
      effective_from: Map.get(map, "effective_from"),
      effective_to: Map.get(map, "effective_to"),
      start_line: Map.fetch!(map, "start_line"),
      source_type: Map.get(map, "source_type"),
      versions: map |> Map.get("versions", []) |> Enum.map(&ShowVersion.from_map/1),
      data:
        map
        |> Map.fetch!("data")
        |> Map.new(fn {key, entry} -> {key, ShowData.from_map(entry)} end),
      rules:
        map
        |> Map.fetch!("rules")
        |> Map.new(fn {key, entry} -> {key, ShowRule.from_map(entry)} end),
      meta: Map.get(map, "meta", %{})
    }
  end
end
