defmodule Lemma.Mcp do
  @moduledoc """
  Pure Lemma MCP tools: engine + JSON args in, catalog or tool result text out.

  Read tools mirror the engine catalog: `evaluate`, `specs`, `show`, `source`,
  `check`, `guide`. Admin/write tools stay in the CLI MCP server only.

  `evaluate/2` always includes explanations and returns formatted ASCII trees
  (plus a Missing data block when inputs are unbound). Args match SDK `evaluate`:
    `spec`, optional `repository`, `rules`, `data` (object), `effective`.
  Do not pass `explain`. User phrasing "ask Lemma" means `evaluate`.
  """

  @type engine :: Lemma.engine()
  @type args :: map()

  @spec list_tools() :: {:ok, [map()]} | {:error, term()}
  def list_tools do
    case Lemma.Native.mcp_list_tools() do
      {:ok, binary} -> {:ok, Jason.decode!(binary)}
      err -> err
    end
  end

  @spec list_resources() :: {:ok, [map()]} | {:error, term()}
  def list_resources do
    case Lemma.Native.mcp_list_resources() do
      {:ok, binary} -> {:ok, Jason.decode!(binary)}
      err -> err
    end
  end

  @spec read_resource(String.t()) :: {:ok, String.t()} | {:error, :unknown_uri, String.t()}
  def read_resource(uri) when is_binary(uri) do
    Lemma.Native.mcp_read_resource(uri)
  end

  @spec evaluate(engine(), args()) ::
          {:ok, String.t()}
          | {:error, :invalid_arguments | :not_found | :diagnostics, String.t()}
  def evaluate(engine, args) when is_map(args) do
    Lemma.Native.mcp_evaluate(engine, Jason.encode!(args))
  end

  @deprecated "Use evaluate/2"
  @spec run(engine(), args()) ::
          {:ok, String.t()}
          | {:error, :invalid_arguments | :not_found | :diagnostics, String.t()}
  def run(engine, args) when is_map(args) do
    evaluate(engine, args)
  end

  @spec specs(engine(), args()) ::
          {:ok, String.t()}
          | {:error, :invalid_arguments | :not_found | :diagnostics, String.t()}
  def specs(engine, args) when is_map(args) do
    Lemma.Native.mcp_specs(engine, Jason.encode!(args))
  end

  @spec show(engine(), args()) ::
          {:ok, String.t()}
          | {:error, :invalid_arguments | :not_found | :diagnostics, String.t()}
  def show(engine, args) when is_map(args) do
    Lemma.Native.mcp_show(engine, Jason.encode!(args))
  end

  @spec source(engine(), args()) ::
          {:ok, String.t()}
          | {:error, :invalid_arguments | :not_found | :diagnostics, String.t()}
  def source(engine, args) when is_map(args) do
    Lemma.Native.mcp_source(engine, Jason.encode!(args))
  end

  @spec check(args()) ::
          {:ok, String.t()}
          | {:error, :invalid_arguments | :not_found | :diagnostics, String.t()}
  def check(args) when is_map(args) do
    Lemma.Native.mcp_check(Jason.encode!(args))
  end

  @spec guide(args()) ::
          {:ok, String.t()}
          | {:error, :invalid_arguments | :not_found | :diagnostics, String.t()}
  def guide(args) when is_map(args) do
    Lemma.Native.mcp_guide(Jason.encode!(args))
  end
end
