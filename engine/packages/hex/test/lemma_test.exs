defmodule LemmaTest do
  use ExUnit.Case, async: true

  @simple_spec """
  spec pricing
  data quantity: number
  data price: 10
  rule total: quantity * price
  rule discount: 0
    unless quantity >= 10 then 5
    unless quantity >= 50 then 15
  """

  @embedded_repo "lemma"

  defp embedded_stdlib_group?(group) do
    group["repository"] == @embedded_repo
  end

  defp workspace_groups(groups) do
    Enum.reject(groups, &embedded_stdlib_group?/1)
  end

  defp embedded_stdlib_group(groups) do
    Enum.find(groups, &embedded_stdlib_group?/1)
  end

  defp spec_count(groups) do
    groups |> Enum.map(fn g -> length(g["specs"]) end) |> Enum.sum()
  end

  defp workspace_spec_count(groups) do
    groups |> workspace_groups() |> spec_count()
  end

  defp date_iso(nil), do: nil

  defp date_iso(iso) when is_binary(iso), do: iso

  # Explanation trees embed source paths (e.g. original vs formatted); compare evaluation payloads only.
  defp comparable_rule_result(rule) when is_map(rule) do
    Map.drop(rule, ["explanation"])
  end

  describe "new/0" do
    test "creates engine with default limits" do
      assert {:ok, engine} = Lemma.new()
      assert is_reference(engine)
    end

    test "creates engine with custom limits" do
      assert {:ok, engine} = Lemma.new(%{"max_sources" => 50})
      assert is_reference(engine)
    end

    test "creates engine with max_normal_form_depth" do
      assert {:ok, engine} = Lemma.new(%{"max_normal_form_depth" => 99})
      assert {:ok, limits} = Lemma.limits(engine)
      assert limits["max_normal_form_depth"] == 99
    end

    test "creates engine with nil limits (defaults)" do
      assert {:ok, engine} = Lemma.new(nil)
      assert is_reference(engine)
    end
  end

  describe "limits/1" do
    test "returns configured limits" do
      assert {:ok, engine} = Lemma.new(%{"max_sources" => 12})
      assert {:ok, limits} = Lemma.limits(engine)
      assert limits["max_sources"] == 12
    end
  end

  describe "snapshot/1 and from_snapshot/1" do
    test "round-trips list and run; corrupt bytes error" do
      assert {:ok, source} = Lemma.new()

      :ok =
        Lemma.load(source, """
        spec snap_demo
        data x: number
        rule y: x + 1
        """)

      assert {:ok, bytes} = Lemma.snapshot(source)
      assert is_binary(bytes)
      assert byte_size(bytes) > 0

      assert {:ok, restored} = Lemma.from_snapshot(bytes)
      assert {:ok, list_source} = Lemma.list(source)
      assert {:ok, list_restored} = Lemma.list(restored)
      assert list_restored == list_source

      assert {:ok, run_source} = Lemma.run(source, %{spec: "snap_demo"}, %{data: %{"x" => 41}})

      assert {:ok, run_restored} =
               Lemma.run(restored, %{spec: "snap_demo"}, %{data: %{"x" => 41}})

      assert comparable_rule_result(run_restored["results"]["y"]) ==
               comparable_rule_result(run_source["results"]["y"])

      corrupt =
        case bytes do
          <<_::8, rest::binary>> -> <<0x58, rest::binary>>
        end

      assert {:error, err} = Lemma.from_snapshot(corrupt)
      assert is_map(err)
    end
  end

  describe "quality/1" do
    test "clean spec has no recommendations" do
      {:ok, engine} = Lemma.new()

      :ok =
        Lemma.load(engine, """
        spec pricing 2026-01-01
        \"\"\"
        Bulk pricing.
        \"\"\"

        data qty: number
          -> minimum 0
          -> maximum 1000000
          -> help "Order quantity."

        rule total: qty
        """)

      assert {:ok, []} = Lemma.quality(engine)
    end

    test "reports missing help with effective_from and source" do
      {:ok, engine} = Lemma.new()

      :ok =
        Lemma.load(engine, """
        spec pricing 2026-01-01
        \"\"\"
        Bulk.
        \"\"\"

        data qty: number
        rule total: qty
        """)

      assert {:ok, recs} = Lemma.quality(engine)
      hit = Enum.find(recs, fn r -> String.contains?(r["message"], "no `-> help`") end)
      assert hit
      assert hit["spec"] == "pricing"
      assert hit["effective_from"] == "2026-01-01"
      assert is_map(hit["source"])
      assert is_binary(hit["source"]["attribute"])
      assert is_integer(hit["source"]["line"])
    end
  end

  describe "update/3" do
    test "replaces a temporal spec slice" do
      {:ok, engine} = Lemma.new()

      :ok =
        Lemma.load(engine, """
        spec pricing
        data quantity: 1
        rule total: quantity * 10
        """)

      :ok =
        Lemma.update(engine, nil, """
        spec pricing
        data quantity: 1
        rule total: quantity * 20
        """)

      assert {:ok, response} = Lemma.run(engine, %{spec: "pricing"}, %{data: %{}})
      assert response["results"]["total"]["number"] == "20"
    end
  end

  describe "new/1 error cases" do
    test "rejects non-integer limit value" do
      assert_raise ErlangError, fn ->
        Lemma.new(%{"max_sources" => "not_a_number"})
      end
    end

    test "rejects unknown limit key" do
      assert_raise ErlangError, fn ->
        Lemma.new(%{"bogus_key" => 10})
      end
    end

    test "rejects negative limit value" do
      assert_raise ErlangError, fn ->
        Lemma.new(%{"max_sources" => -1})
      end
    end
  end

  describe "new/1 max_normalized_expression_nodes" do
    test "accepts max_normalized_expression_nodes limit" do
      assert {:ok, engine} = Lemma.new(%{"max_normalized_expression_nodes" => 1000})
      assert is_reference(engine)
    end

    test "enforces max_normalized_expression_nodes during planning" do
      # Wide unless over distinct data — many unique NormalForm cells, no Rule-overlay
      # sharing. Shared self-doubling chains stay linear and no longer trip this limit.
      arm_count = 40

      data =
        Enum.map_join(0..(arm_count - 1), "\n", fn i -> "data d#{i}: boolean" end)

      arms =
        Enum.map_join(0..(arm_count - 1), "\n", fn i -> "  unless d#{i} then #{i}" end)

      blowup = """
      spec blowup
      #{data}
      rule r: 0
      #{arms}
      """

      {:ok, engine} = Lemma.new(%{"max_normalized_expression_nodes" => 50})
      result = Lemma.load(engine, %{"blowup.lemma" => blowup})
      assert {:error, errors} = result
      assert is_list(errors)

      error = hd(errors)
      assert error[:kind] == "resource_limit"
      assert error[:message] =~ "expression nodes" or error[:message] =~ "normal-form"
      assert is_binary(error[:limit_name])
      assert is_binary(error[:limit_value])
      assert is_binary(error[:actual_value])
    end
  end

  describe "load/2 binary" do
    test "loads inline volatile source" do
      {:ok, engine} = Lemma.new()
      assert :ok = Lemma.load(engine, @simple_spec)
    end

    test "returns errors for invalid inline source" do
      {:ok, engine} = Lemma.new()
      assert {:error, errors} = Lemma.load(engine, "spec bad\ndata x: [bogus]")
      assert is_list(errors)
      assert length(errors) > 0
      first = hd(errors)
      assert is_map(first)
      assert Map.has_key?(first, :message)
      assert first[:kind] == "parsing"
      # Unified error API: `source` (not Hex-only `location`), with attribute + length.
      assert Map.has_key?(first, :source), "error must use source key, not location"
      refute Map.has_key?(first, :location)
      assert is_map(first[:source])
      assert is_binary(first[:source][:attribute])
      assert is_integer(first[:source][:length])
      assert Map.has_key?(first, :related_data)
      assert Map.has_key?(first, :spec)
      assert Map.has_key?(first, :related_spec)
    end
  end

  describe "load/2 labeled" do
    test "loads a labeled spec from a map" do
      {:ok, engine} = Lemma.new()
      assert :ok = Lemma.load(engine, %{"pricing.lemma" => @simple_spec})
    end

    test "loads from a list of label-code tuples" do
      {:ok, engine} = Lemma.new()

      assert :ok =
               Lemma.load(engine, [
                 {"pricing.lemma", @simple_spec}
               ])
    end

    test "path label volatile loads as Path not Volatile" do
      {:ok, engine} = Lemma.new()

      assert :ok =
               Lemma.load(engine, %{
                 "volatile" => "spec inline_test\ndata x: 1\nrule y: x + 1"
               })

      {:ok, show} = Lemma.show(engine, nil, "inline_test")
      assert show["source_type"] == %{"path" => "volatile"}
    end

    test "rejects empty source label" do
      {:ok, engine} = Lemma.new()

      assert {:error, errors} =
               Lemma.load(engine, %{
                 "" => "spec inline_test\ndata x: 1\nrule y: x + 1"
               })

      assert hd(errors)[:kind] == "request"
    end

    test "list of tuples preserves caller order in list/1" do
      {:ok, engine} = Lemma.new()

      assert :ok =
               Lemma.load(engine, [
                 {"zebra.lemma", "spec zebra\ndata n: 1\nrule r: n"},
                 {"alpha.lemma", "spec alpha\ndata n: 1\nrule r: n"},
                 {"mike.lemma", "spec mike\ndata n: 1\nrule r: n"}
               ])

      assert {:ok, groups} = Lemma.list(engine)
      [workspace] = workspace_groups(groups)
      names = Enum.map(workspace["specs"], & &1["name"])
      assert names == ["zebra", "alpha", "mike"]
    end

    test "list of tuples preserves caller order in parse errors" do
      {:ok, engine} = Lemma.new()

      assert {:error, errors} =
               Lemma.load(engine, [
                 {"zebra.lemma", "this is not lemma"},
                 {"yankee.lemma", "spec ok\nrule r: 1\n"},
                 {"xray.lemma", "also not lemma"}
               ])

      attrs =
        errors
        |> Enum.map(fn err -> get_in(err, [:source, :attribute]) end)
        |> Enum.reject(&is_nil/1)

      assert attrs == ["zebra.lemma", "xray.lemma"]
    end

    test "map load orders sources lexicographically by label in list/1" do
      {:ok, engine} = Lemma.new()

      # Literal key order is non-alphabetical; contract is lexicographic labels.
      assert :ok =
               Lemma.load(engine, %{
                 "zebra.lemma" => "spec zebra\ndata n: 1\nrule r: n",
                 "alpha.lemma" => "spec alpha\ndata n: 1\nrule r: n",
                 "mike.lemma" => "spec mike\ndata n: 1\nrule r: n"
               })

      assert {:ok, groups} = Lemma.list(engine)
      [workspace] = workspace_groups(groups)
      names = Enum.map(workspace["specs"], & &1["name"])
      assert names == ["alpha", "mike", "zebra"]
    end

    test "map load orders parse errors lexicographically by label" do
      {:ok, engine} = Lemma.new()

      assert {:error, errors} =
               Lemma.load(engine, %{
                 "zebra.lemma" => "this is not lemma",
                 "yankee.lemma" => "spec ok\nrule r: 1\n",
                 "xray.lemma" => "also not lemma"
               })

      attrs =
        errors
        |> Enum.map(fn err -> get_in(err, [:source, :attribute]) end)
        |> Enum.reject(&is_nil/1)

      assert attrs == ["xray.lemma", "zebra.lemma"]
    end
  end

  describe "list/1" do
    test "lists loaded specs with metadata" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"pricing.lemma" => @simple_spec})
      assert {:ok, groups} = Lemma.list(engine)
      assert is_list(groups)
      assert length(workspace_groups(groups)) == 1
      group = hd(workspace_groups(groups))
      assert group["repository"] == nil
      assert length(group["specs"]) == 1
      spec = hd(group["specs"])
      assert spec["name"] == "pricing"
      refute Map.has_key?(spec, "start_line")
      refute Map.has_key?(spec, "source_type")

      {:ok, show} = Lemma.show(engine, nil, "pricing")
      assert show["start_line"] == 1
      assert show["source_type"] == %{"path" => "pricing.lemma"}
    end

    test "fresh engine lists embedded stdlib repository" do
      {:ok, engine} = Lemma.new()
      {:ok, groups} = Lemma.list(engine)
      embedded = embedded_stdlib_group(groups)
      assert embedded != nil
      assert embedded["repository"] == @embedded_repo
      assert hd(embedded["specs"])["name"] == "units"
    end

    test "effective_from is nil when not set" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"test.lemma" => "spec no_effective\ndata x: 1"})
      {:ok, groups} = Lemma.list(engine)
      [group] = workspace_groups(groups)
      spec = hd(group["specs"])
      assert spec["effective_from"] == nil
    end

    test "effective_to is nil for an unversioned spec (no successor)" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"test.lemma" => "spec no_effective\ndata x: 1"})
      {:ok, groups} = Lemma.list(engine)
      [group] = workspace_groups(groups)
      spec = hd(group["specs"])
      assert spec["effective_to"] == nil
    end

    test "effective_to equals the next version's effective_from for earlier rows" do
      {:ok, engine} = Lemma.new()

      code = """
      spec pricing 2025-01-01
      data base: 10
      rule total: base

      spec pricing 2026-01-01
      data base: 99
      rule total: base
      """

      :ok = Lemma.load(engine, %{"temporal.lemma" => code})
      {:ok, groups} = Lemma.list(engine)
      assert length(workspace_groups(groups)) == 1
      entries = hd(workspace_groups(groups))["specs"]
      assert length(entries) == 2

      [earlier, latest] = entries
      assert date_iso(earlier["effective_from"]) == "2025-01-01"
      assert date_iso(earlier["effective_to"]) == "2026-01-01"
      assert date_iso(latest["effective_from"]) == "2026-01-01"
      assert latest["effective_to"] == nil
    end
  end

  describe "source/4" do
    test "returns embedded lemma repo source" do
      {:ok, engine} = Lemma.new()
      assert {:ok, source} = Lemma.source(engine, @embedded_repo, nil, nil)
      assert source =~ "spec units"
      assert source =~ "trait duration"
    end

    test "nil repository returns default repository source after load" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"ws.lemma" => @simple_spec})
      assert {:ok, source} = Lemma.source(engine, nil, nil, nil)
      assert source =~ "spec pricing"
    end

    test "spec slice source" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"ws.lemma" => @simple_spec})
      assert {:ok, source} = Lemma.source(engine, nil, "pricing", nil)
      assert source =~ "spec pricing"
    end

    test "unknown qualifier returns error" do
      {:ok, engine} = Lemma.new()
      assert {:error, _} = Lemma.source(engine, "workspace", nil, nil)
    end
  end

  describe "show/4" do
    test "returns show for loaded spec with ShowData + kind-tagged types" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"pricing.lemma" => @simple_spec})
      assert {:ok, show} = Lemma.show(engine, nil, "pricing")
      assert is_map(show)
      assert show["spec"] == "pricing"
      assert is_map(show["data"])
      assert is_map(show["rules"])
      assert Map.has_key?(show["data"], "quantity")
      assert Map.has_key?(show["rules"], "total")
      assert Map.has_key?(show["rules"], "discount")

      quantity = show["data"]["quantity"]
      assert is_map(quantity), "ShowData is a named object, not a tuple"
      assert is_map(quantity["type"])
      assert is_binary(quantity["type"]["kind"]), "type carries `kind` discriminator"
    end

    test "returns error for unknown spec" do
      {:ok, engine} = Lemma.new()
      assert {:error, _} = Lemma.show(engine, nil, "nonexistent")
    end
  end

  describe "run/3" do
    test "runs spec with provided data" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"pricing.lemma" => @simple_spec})

      assert {:ok, response} =
               Lemma.run(engine, %{spec: "pricing"}, %{data: %{"quantity" => "5"}})

      assert is_map(response)
      assert response["spec"] == "pricing"
      refute Map.has_key?(response, "data")
      results = response["results"]
      assert is_map(results)
      total = results["total"]
      assert total["display"] == "50"
      assert total["number"] == "50"
      refute Map.has_key?(total, "missing_data")

      typed = Lemma.Response.from_map(response)
      assert %Lemma.Response{spec: "pricing"} = typed

      assert %Lemma.RuleResult{display: "50", number: "50", vetoed: false} =
               Map.fetch!(typed.results, "total")
    end

    test "exposes per-rule missing_data when inputs unbound" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"pricing.lemma" => @simple_spec})

      assert {:ok, response} = Lemma.run(engine, %{spec: "pricing"}, %{})

      refute Map.has_key?(response, "data")
      total = response["results"]["total"]
      assert is_list(total["missing_data"])
      assert "quantity" in total["missing_data"]
    end

    test "runs spec with measure triggering unless clause" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"pricing.lemma" => @simple_spec})

      {:ok, response} =
        Lemma.run(engine, %{spec: "pricing"}, %{data: %{"quantity" => "10"}})

      results = response["results"]
      assert results["discount"]["display"] == "5"
      assert results["discount"]["number"] == "5"
    end

    test "runs spec with no optional data" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"s.lemma" => "spec simple\ndata x: 1\nrule y: x + 1"})
      {:ok, response} = Lemma.run(engine, %{spec: "simple"})
      results = response["results"]
      assert results["y"]["display"] == "2"
      assert results["y"]["number"] == "2"
    end

    test "returns error for unknown spec" do
      {:ok, engine} = Lemma.new()
      assert {:error, _} = Lemma.run(engine, %{spec: "nonexistent"})
    end
  end

  describe "remove/3" do
    test "removes a loaded spec" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, %{"rm.lemma" => "spec removable\ndata x: 1\nrule y: x + 1"})
      {:ok, groups} = Lemma.list(engine)
      assert workspace_spec_count(groups) == 1

      assert :ok = Lemma.remove(engine, nil, "removable", "2025-01-01")

      {:ok, specs} = Lemma.list(engine)
      assert workspace_spec_count(specs) == 0
      assert embedded_stdlib_group(specs) != nil
    end

    test "returns error for unknown spec" do
      {:ok, engine} = Lemma.new()
      assert {:error, _} = Lemma.remove(engine, nil, "ghost", "2025-01-01")
    end
  end

  describe "multiple engines" do
    test "engines are independent" do
      {:ok, e1} = Lemma.new()
      {:ok, e2} = Lemma.new()
      :ok = Lemma.load(e1, %{"a.lemma" => "spec a\ndata x: 1\nrule y: x + 1"})
      {:ok, groups1} = Lemma.list(e1)
      {:ok, groups2} = Lemma.list(e2)
      assert workspace_spec_count(groups1) == 1
      assert workspace_spec_count(groups2) == 0
      assert embedded_stdlib_group(groups2) != nil
    end
  end

  describe "format/1" do
    test "formats valid lemma source" do
      input = "spec foo\ndata   x:  1\nrule y: x +  1"
      assert {:ok, formatted} = Lemma.format(input)
      assert is_binary(formatted)
      assert formatted =~ "spec foo"
      assert formatted =~ "data x"
      assert formatted =~ "rule y:"
      assert formatted =~ "x + 1"
    end

    test "returns error for invalid source" do
      assert {:error, err} = Lemma.format("not valid lemma at all !!!")
      assert is_map(err)
      assert Map.has_key?(err, :message)
      assert err[:kind] == "parsing"
      typed = Lemma.EngineError.from_map(err)
      assert %Lemma.EngineError{kind: "parsing"} = typed
      assert is_binary(typed.message)
    end

    test "parses api fixtures into typed show shape" do
      fixture_path =
        Path.expand("../../../tests/fixtures/api/show_minimal.json", __DIR__)

      assert {:ok, raw} = File.read(fixture_path)
      assert {:ok, show_map} = Jason.decode(raw)
      assert is_binary(show_map["spec"])
      assert is_binary(show_map["effective_from"])
      assert is_map(show_map["data"])
      assert is_map(show_map["rules"])

      show = Lemma.Show.from_map(show_map)
      assert %Lemma.Show{} = show
      assert show.spec == show_map["spec"]
      assert show.effective_from == show_map["effective_from"]
      assert %Lemma.ShowVersion{effective_from: "2024-01-01"} = hd(show.versions)

      amount = Map.fetch!(show.data, "amount")
      assert %Lemma.ShowData{} = amount
      assert amount.type["kind"] == "number"
      assert amount.suggestion == %{"number" => "1"}
      assert amount.needed_by_rules == ["ok"]

      ok = Map.fetch!(show.rules, "ok")
      assert %Lemma.ShowRule{} = ok
      assert ok.type["kind"] == "number"
      assert ok.depends_on_rules == []
      assert length(ok.branches) == 1
    end

    test "preserves semantics after formatting" do
      input = "spec fmt\ndata x: number\nrule y: x *   2\nrule z: y + 1"
      {:ok, formatted} = Lemma.format(input)

      {:ok, e1} = Lemma.new()
      {:ok, e2} = Lemma.new()
      :ok = Lemma.load(e1, %{"original" => input})
      :ok = Lemma.load(e2, %{"formatted" => formatted})

      {:ok, r1} = Lemma.run(e1, %{spec: "fmt"}, %{data: %{"x" => "5"}})
      {:ok, r2} = Lemma.run(e2, %{spec: "fmt"}, %{data: %{"x" => "5"}})

      assert comparable_rule_result(r1["results"]["y"]) ==
               comparable_rule_result(r2["results"]["y"])

      assert comparable_rule_result(r1["results"]["z"]) ==
               comparable_rule_result(r2["results"]["z"])
    end
  end

  describe "mcp" do
    test "list_tools includes run evaluate list show source check guide" do
      assert {:ok, tools} = Lemma.Mcp.list_tools()
      names = Enum.map(tools, & &1["name"])

      assert names == ["run", "evaluate", "list", "show", "source", "check", "guide"]
      refute "add_spec" in names
      assert hd(tools)["inputSchema"]["required"] == ["spec"]
      assert Enum.at(tools, 1)["name"] == "evaluate"
      assert Enum.at(tools, 1)["description"] =~ "Deprecated"
    end

    test "run and list on a loaded engine" do
      {:ok, engine} = Lemma.new()
      :ok = Lemma.load(engine, @simple_spec)

      assert {:ok, text} =
               Lemma.Mcp.run(engine, %{
                 "spec" => "pricing",
                 "rules" => "total",
                 "data" => %{"quantity" => 3}
               })

      assert text =~ "total: 30"

      assert {:ok, alias_text} =
               Lemma.Mcp.evaluate(engine, %{
                 "spec" => "pricing",
                 "rules" => "total",
                 "data" => %{"quantity" => 3}
               })

      assert alias_text == text

      assert {:ok, list} = Lemma.Mcp.list(engine, %{})
      assert list =~ "pricing"
    end

    test "check success returns quality JSON array" do
      assert {:ok, text} =
               Lemma.Mcp.check(%{
                 "sources" => [["ok.lemma", "spec ok\nrule r: 1\n"]]
               })

      recs = Jason.decode!(text)
      assert is_list(recs)
    end

    test "check diagnostics for invalid source" do
      assert {:error, :diagnostics, json} =
               Lemma.Mcp.check(%{"sources" => [["bad.lemma", "not lemma"]]})

      assert Jason.decode!(json) |> is_list()
    end

    test "guide default is evaluate guide" do
      assert {:ok, text} = Lemma.Mcp.guide(%{})
      assert text =~ "Talk like a consultant"
      assert text =~ "`run`"
    end
  end

  test "install rejects empty LemmaBase id" do
    {:ok, engine} = Lemma.new()
    assert {:error, errors} = Lemma.install(engine, "   ")

    assert Enum.any?(errors, fn err ->
             (err["kind"] || err[:kind]) == "registry"
           end)
  end

  describe "install with injected transport" do
    defp fixtures_dir do
      Path.expand("../../../tests/registry_fixtures", __DIR__)
    end

    defp fixture_transport(expected_url, status, body) do
      fn url, _headers ->
        assert url == expected_url
        {:ok, status, [], body}
      end
    end

    test "successful fetch returns source and id" do
      body = File.read!(Path.join(fixtures_dir(), "@iso/countries.lemma"))
      transport = fixture_transport("https://lemmabase.com/@iso/countries.lemma", 200, body)
      {:ok, engine} = Lemma.new()

      assert {:ok, result} = Lemma.install(engine, "@iso/countries", transport)
      assert result["id"] == "@iso/countries"
      assert is_binary(result["source"])
      assert result["source"] =~ "spec alpha2"
    end

    test "404 returns registry error" do
      transport =
        fixture_transport("https://lemmabase.com/@iso/missing.lemma", 404, "not found")

      {:ok, engine} = Lemma.new()
      assert {:error, errors} = Lemma.install(engine, "@iso/missing", transport)
      assert is_list(errors)
      assert Enum.any?(errors, fn err -> err[:kind] == "registry" end)
    end

    test "transport error returns registry network error" do
      transport = fn url, _headers ->
        assert url == "https://lemmabase.com/@iso/countries.lemma"
        {:error, "connection refused"}
      end

      {:ok, engine} = Lemma.new()
      assert {:error, errors} = Lemma.install(engine, "@iso/countries", transport)
      assert is_list(errors)
      assert Enum.any?(errors, fn err -> err[:registry_kind] == "network_error" end)
    end
  end
end
