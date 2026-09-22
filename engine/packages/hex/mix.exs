defmodule Lemma.MixProject do
  use Mix.Project

  @version "0.9.11"
  @source_url "https://github.com/lemma/lemma"

  def project do
    [
      app: :lemma_engine,
      version: @version,
      elixir: "~> 1.14",
      compilers: Mix.compilers(),
      start_permanent: Mix.env() == :prod,
      aliases: aliases(),
      deps: deps(),
      description: "Lemma rules engine for Elixir",
      package: package(),
      docs: docs()
    ]
  end

  def application do
    []
  end

  def cli do
    [
      preferred_envs: [
        precommit: :test,
        "test.precommit": :test
      ]
    ]
  end

  defp aliases do
    [
      precommit: [
        "format --check-formatted",
        "deps.get --check-locked",
        "hex.audit",
        "compile",
        "test.precommit"
      ],
      "test.precommit": ["test"]
    ]
  end

  defp deps do
    [
      {:jason, "~> 1.4"},
      {:req, "~> 0.5"},
      {:rustler_precompiled, "~> 0.9"},
      {:rustler, "~> 0.38", optional: true},
      {:ex_doc, "~> 0.40.3", only: :dev, runtime: false}
    ]
  end

  defp package do
    [
      name: "lemma_engine",
      files: [
        "lib",
        "native/lemma_hex/src",
        "native/lemma_hex/Cargo*",
        "checksum-*.exs",
        "mix.exs",
        "README.md"
      ],
      licenses: ["Apache-2.0"],
      links: %{"GitHub" => @source_url}
    ]
  end

  defp docs do
    [
      main: "Lemma",
      source_url: @source_url,
      extras: ["README.md"]
    ]
  end
end
