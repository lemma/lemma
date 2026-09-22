defmodule Lemma.RuleResult do
  @moduledoc """
  One rule result from `Lemma.run/3`, as returned in `Lemma.Response.results`.

  Value fields (`measure`, `ratio`, `number`, …) mirror the flattened Rust
  `RuleResultValue`. `explanation` is the raw explanation JSON map when present
  (tagged by `"type"`); pattern-match on that tag directly.
  """

  @type t :: %__MODULE__{
          vetoed: boolean(),
          result: String.t() | nil,
          veto_reason: String.t() | nil,
          rule_type: String.t(),
          measure: %{optional(String.t()) => String.t()} | nil,
          ratio: %{optional(String.t()) => String.t()} | nil,
          number: String.t() | nil,
          boolean: boolean() | nil,
          text: String.t() | nil,
          date: String.t() | nil,
          time: String.t() | nil,
          calendar: map() | nil,
          range: map() | nil,
          missing_data: [String.t()] | nil,
          explanation: map() | nil
        }

  @enforce_keys [:vetoed, :rule_type]
  defstruct [
    :vetoed,
    :result,
    :veto_reason,
    :rule_type,
    :measure,
    :ratio,
    :number,
    :boolean,
    :text,
    :date,
    :time,
    :calendar,
    :range,
    :missing_data,
    :explanation
  ]

  @doc """
  Builds a `Lemma.RuleResult` from one value of the decoded `Lemma.run/3` JSON
  `"results"` map. Optional fields are absent (not `null`) from the API JSON when
  unset; `Map.get/3` yields `nil` for those.
  """
  @spec from_map(map()) :: t()
  def from_map(map) when is_map(map) do
    %__MODULE__{
      vetoed: Map.fetch!(map, "vetoed"),
      result: Map.get(map, "result"),
      veto_reason: Map.get(map, "veto_reason"),
      rule_type: Map.fetch!(map, "rule_type"),
      measure: Map.get(map, "measure"),
      ratio: Map.get(map, "ratio"),
      number: Map.get(map, "number"),
      boolean: Map.get(map, "boolean"),
      text: Map.get(map, "text"),
      date: Map.get(map, "date"),
      time: Map.get(map, "time"),
      calendar: Map.get(map, "calendar"),
      range: Map.get(map, "range"),
      missing_data: Map.get(map, "missing_data"),
      explanation: Map.get(map, "explanation")
    }
  end
end
