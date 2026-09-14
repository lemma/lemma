defmodule Lemma do
  @moduledoc """
  Lemma rules engine for Elixir.

  Wraps the Lemma engine (Rust) via NIFs. Create an engine, load sources,
  run evaluations, and show specs.

  ## Example

      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, "spec foo\\ndata x: 1\\nrule y: x + 1")
      {:ok, response} = Lemma.run(engine, %{spec: "foo"}, %{data: %{}})
      # response is a map from decoded JSON

  ## Engine lifecycle

  Each engine is an opaque resource. Do not share the same engine ref across
  processes unless you serialize access (e.g. via a GenServer).
  """

  @type engine :: reference()
  @type spec_name :: String.t()
  @type limits_map :: %{String.t() => pos_integer()} | nil
  @type repository :: String.t() | nil

  @doc """
  Creates a new engine. Optionally pass a map of resource limits; omitted keys use defaults.
  """
  @spec new(limits_map) :: {:ok, engine()} | {:error, term()}
  def new(limits \\ nil) do
    Lemma.Native.lemma_new(limits)
  end

  @doc """
  Returns the engine's configured resource limits.
  """
  @spec limits(engine()) :: {:ok, map()} | {:error, term()}
  def limits(engine) do
    case Lemma.Native.lemma_limits(engine) do
      {:ok, binary} -> {:ok, Jason.decode!(binary)}
      err -> err
    end
  end

  @doc """
  Persist parsed specs + plans + limits as opaque bytes. Restore with `from_snapshot/1`.
  """
  @spec snapshot(engine()) :: {:ok, binary()} | {:error, map()}
  def snapshot(engine) do
    Lemma.Native.lemma_snapshot(engine)
  end

  @doc """
  Restore an engine from `snapshot/1` bytes.
  """
  @spec from_snapshot(binary()) :: {:ok, engine()} | {:error, map()}
  def from_snapshot(bytes) when is_binary(bytes) do
    Lemma.Native.lemma_from_snapshot(bytes)
  end

  @doc """
  Structural quality recommendations across loaded specs. Advisory only.
  """
  @spec quality(engine()) :: {:ok, [map()]} | {:error, term()}
  def quality(engine) do
    case Lemma.Native.lemma_quality(engine) do
      {:ok, binary} -> {:ok, Jason.decode!(binary)}
      err -> err
    end
  end

  @doc """
  Loads Lemma source(s).

  - binary → one volatile source in the default repository
  - `[{label, code}, ...]` → labeled sources in caller list order
  - map → labeled sources in lexicographic label order (BEAM maps have no insertion order)
  """
  @spec load(engine(), String.t()) :: :ok | {:error, [map()]}
  @spec load(engine(), map() | [{String.t(), String.t()}]) :: :ok | {:error, [map()]}
  def load(engine, code) when is_binary(code) do
    Lemma.Native.lemma_load(engine, code)
  end

  def load(engine, sources) when is_map(sources) or is_list(sources) do
    Lemma.Native.lemma_load(engine, sources)
  end

  @doc """
  Downloads a repository from LemmaBase.

  Returns `{:ok, %{source: source, id: id}}` (atom keys). Does not load the engine
  and does not write `lemma_deps/`. Call `load/2` with the returned id as the
  source label before loading workspace specs that `uses` it.

  `transport` is a 2-arity function `(url, headers) -> {:ok, status, headers, body} | {:error, message}`.
  Default is `Lemma.Transport.get/2` (Req).
  """
  @spec install(engine(), String.t()) :: {:ok, map()} | {:error, [map()]}
  @spec install(engine(), String.t(), (String.t(), [{String.t(), String.t()}] -> term())) ::
          {:ok, map()} | {:error, [map()]}
  def install(engine, repository, transport \\ &Lemma.Transport.get/2)

  def install(engine, repository, transport)
      when is_binary(repository) and is_function(transport, 2) do
    install_step(Lemma.Native.lemma_install_start(engine, repository), transport)
  end

  defp install_step({:fetch, url, headers, install}, transport) do
    response = transport.(url, headers)
    install_step(Lemma.Native.lemma_install_respond(install, response), transport)
  end

  defp install_step({:finished, {:ok, json}}, _transport) when is_binary(json) do
    {:ok, Jason.decode!(json)}
  end

  defp install_step({:finished, {:error, errors}}, _transport), do: {:error, errors}

  defp install_step(other, _transport) do
    raise "BUG: unknown install step: #{inspect(other)}"
  end

  @doc """
  Lists loaded specs grouped by repository (metadata only).
  """
  @spec list(engine()) :: {:ok, [map()]} | {:error, term()}
  def list(engine) do
    case Lemma.Native.lemma_list(engine) do
      {:ok, binary} -> {:ok, Jason.decode!(binary)}
      err -> err
    end
  end

  @doc """
  Returns spec interface and temporal window at `effective`.

  Positional args: `repository`, `spec`, `effective`.
  """
  @spec show(engine(), repository(), spec_name(), String.t() | nil) ::
          {:ok, map()} | {:error, term()}
  def show(engine, repository, spec, effective \\ nil) do
    case Lemma.Native.lemma_show(engine, repository, spec, effective) do
      {:ok, binary} -> {:ok, Jason.decode!(binary)}
      err -> err
    end
  end

  @doc """
  Returns formatted canonical Lemma source.

  Omit `spec` for whole-repository text. When `spec` is set, `effective` selects the slice.
  """
  @spec source(engine(), repository(), spec_name() | nil, String.t() | nil) ::
          {:ok, String.t()} | {:error, term()}
  def source(engine, repository, spec \\ nil, effective \\ nil) do
    Lemma.Native.lemma_source(engine, repository, spec, effective)
  end

  @doc """
  Runs a spec.

  `target` is `%{repo: repository | nil, spec: name, effective: datetime | nil}`.
  `options` is `%{data: map, rules: [String.t()] | nil, explain: boolean}` (defaults apply when omitted).

  Each rule result may include `missing_data` (unbound input keys as strings). Types,
  filled literals, and suggestions are on `show/4` only — not on the evaluate response.
  """
  @spec run(engine(), map(), map()) :: {:ok, map()} | {:error, term()}
  def run(engine, target, options \\ %{}) do
    case Lemma.Native.lemma_run(engine, target, options) do
      {:ok, binary} -> {:ok, Jason.decode!(binary)}
      err -> err
    end
  end

  @doc """
  Removes a temporal spec slice.

  Positional args: `repository`, `spec`, `effective`.
  """
  @spec remove(engine(), repository(), spec_name(), String.t() | nil) ::
          :ok | {:error, term()}
  def remove(engine, repository, spec, effective) do
    Lemma.Native.lemma_remove(engine, repository, spec, effective)
  end

  @doc """
  Replaces identities in `code` (atomic upsert; Path/Dependency prune siblings).

  `attribute` is the source label (path or `@owner/repo`). Omit (`nil`) for a volatile source.
  """
  @spec update(engine(), repository(), String.t(), String.t() | nil) ::
          :ok | {:error, [map()]}
  def update(engine, repository, code, attribute \\ nil) do
    Lemma.Native.lemma_update(engine, repository, code, attribute)
  end

  @doc """
  Formats Lemma source code. Does not require an engine instance.
  """
  @spec format(String.t()) :: {:ok, String.t()} | {:error, term()}
  def format(code) do
    Lemma.Native.lemma_format(code)
  end
end
