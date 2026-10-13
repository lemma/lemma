defmodule Lemma.Native do
  @moduledoc false
  version = Mix.Project.config()[:version]

  use RustlerPrecompiled,
    otp_app: :lemma_engine,
    crate: "lemma_hex",
    base_url: "https://github.com/lemma/lemma/releases/download/lemma-v#{version}",
    force_build: not File.exists?("checksum-Elixir.Lemma.Native.exs"),
    version: version,
    targets: ~w(
      aarch64-apple-darwin
      x86_64-apple-darwin
      aarch64-unknown-linux-gnu
      x86_64-unknown-linux-gnu
      x86_64-pc-windows-msvc
      aarch64-pc-windows-msvc
    )

  def lemma_version, do: :erlang.nif_error(:nif_not_loaded)

  def lemma_new(_limits), do: :erlang.nif_error(:nif_not_loaded)
  def lemma_load(_resource, _sources), do: :erlang.nif_error(:nif_not_loaded)
  def lemma_specs(_resource), do: :erlang.nif_error(:nif_not_loaded)

  def lemma_show(_resource, _repository, _spec, _effective),
    do: :erlang.nif_error(:nif_not_loaded)

  def lemma_source(_resource, _repository, _spec, _effective),
    do: :erlang.nif_error(:nif_not_loaded)

  def lemma_run(_resource, _target, _options), do: :erlang.nif_error(:nif_not_loaded)

  def lemma_remove(_resource, _repository, _spec_name, _effective),
    do: :erlang.nif_error(:nif_not_loaded)

  def lemma_update(_resource, _repository, _code, _attribute),
    do: :erlang.nif_error(:nif_not_loaded)

  def lemma_limits(_resource), do: :erlang.nif_error(:nif_not_loaded)

  def lemma_snapshot(_resource), do: :erlang.nif_error(:nif_not_loaded)

  def lemma_from_snapshot(_bytes), do: :erlang.nif_error(:nif_not_loaded)

  def lemma_quality(_resource), do: :erlang.nif_error(:nif_not_loaded)

  def lemma_format(_code), do: :erlang.nif_error(:nif_not_loaded)

  def lemma_generate_openapi(_resource, _explain, _effective_opt),
    do: :erlang.nif_error(:nif_not_loaded)

  def lemma_temporal_api_sources(_resource), do: :erlang.nif_error(:nif_not_loaded)

  def mcp_list_tools, do: :erlang.nif_error(:nif_not_loaded)
  def mcp_list_resources, do: :erlang.nif_error(:nif_not_loaded)
  def mcp_read_resource(_uri), do: :erlang.nif_error(:nif_not_loaded)
  def mcp_run(_resource, _args_json), do: :erlang.nif_error(:nif_not_loaded)
  def mcp_evaluate(_resource, _args_json), do: :erlang.nif_error(:nif_not_loaded)
  def mcp_specs(_resource, _args_json), do: :erlang.nif_error(:nif_not_loaded)
  def mcp_show(_resource, _args_json), do: :erlang.nif_error(:nif_not_loaded)
  def mcp_source(_resource, _args_json), do: :erlang.nif_error(:nif_not_loaded)
  def mcp_check(_args_json), do: :erlang.nif_error(:nif_not_loaded)
  def mcp_guide(_args_json), do: :erlang.nif_error(:nif_not_loaded)

  def lemma_install_start(_engine, _repository), do: :erlang.nif_error(:nif_not_loaded)
  def lemma_install_respond(_install, _response), do: :erlang.nif_error(:nif_not_loaded)
end
