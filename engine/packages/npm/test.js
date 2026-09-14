#!/usr/bin/env node
/**
 * Node: initSync + Engine. Browser: init + Engine.
 */

import { readFileSync, existsSync } from 'fs';
import { join, dirname, resolve } from 'path';
import { fileURLToPath, pathToFileURL } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const DIST_PATH = join(__dirname, 'dist');

function assert(cond, msg) {
  if (!cond) throw new Error(msg || 'assertion failed');
}

function ruleNumber(rr) {
  if (!rr || rr.number == null) return null;
  return Number(rr.number);
}

function ruleMeasureUnit(rr, unit) {
  if (!rr || !rr.measure || typeof rr.measure !== 'object') return null;
  const v = rr.measure[unit];
  return v != null ? Number(v) : null;
}

function runEx(engine, spec, rules, data, effective, explain = false) {
  try {
    return engine.run({
      spec,
      data,
      rules: rules ?? undefined,
      effective: effective ?? undefined,
      explain,
    });
  } catch (e) {
    throw new Error(formatReject(e));
  }
}

const ERROR_KINDS = new Set([
  'parsing',
  'validation',
  'inversion',
  'registry',
  'request',
  'resource_limit',
]);

function assertEngineError(e) {
  assert(e && typeof e === 'object' && !Array.isArray(e), 'EngineError must be plain object');
  assert(ERROR_KINDS.has(e.kind), `kind must be known, got: ${e.kind}`);
  assert(typeof e.message === 'string', 'message must be string');
  assert(e.related_data === null || typeof e.related_data === 'string', 'related_data string|null');
  assert(e.spec === null || typeof e.spec === 'string', 'spec string|null');
  assert(e.source === null || (e.source && typeof e.source === 'object'), 'source object|null');
}

function formatReject(e) {
  if (Array.isArray(e)) {
    return e.map((it) => (it && it.message) ? it.message : String(it)).join('\n');
  }
  if (e && typeof e === 'object' && typeof e.message === 'string') return e.message;
  return String(e);
}

function assertResponseShape(resp, specName) {
  assert(resp && typeof resp === 'object', 'run() must return object');
  assert(resp.spec === specName, `spec want ${specName}, got ${resp.spec}`);
  assert(typeof resp.effective === 'string', 'effective must be string');
  assert(
    resp.results && typeof resp.results === 'object' && !Array.isArray(resp.results),
    'results must be plain object'
  );
  assert(!Object.prototype.hasOwnProperty.call(resp, 'data'), 'top-level data must be absent');
  for (const [ruleName, rule] of Object.entries(resp.results)) {
    assert(rule && typeof rule === 'object' && !Array.isArray(rule), `results.${ruleName} must be object`);
    if (Object.prototype.hasOwnProperty.call(rule, 'missing_data')) {
      assert(
        Array.isArray(rule.missing_data) && rule.missing_data.every((k) => typeof k === 'string'),
        `results.${ruleName}.missing_data must be string[]`
      );
    }
  }
}

async function case_(name, fn) {
  const t0 = performance.now();
  try {
    await fn();
    console.log(`  ok  ${name} (${(performance.now() - t0).toFixed(1)}ms)`);
  } catch (e) {
    console.error(`  FAIL ${name}:`, e.message || e);
    throw e;
  }
}

function flattenListGroups(groups) {
  const out = [];
  for (const g of groups) {
    const repository = g.repository;
    for (const lemmaSpec of g.specs) {
      out.push({ ...lemmaSpec, repository });
    }
  }
  return out;
}

function lemmaSpecSourcePathIncludes(lemmaSpec, needle) {
  const st =
    lemmaSpec && typeof lemmaSpec === 'object' && lemmaSpec.source_type != null
      ? lemmaSpec.source_type
      : null;
  if (st == null) return false;
  const s = JSON.stringify(st);
  return typeof s === 'string' && s.includes(needle);
}

function specNames(listGroups) {
  return flattenListGroups(listGroups)
    .map((e) => e && e.name)
    .filter(Boolean);
}

function failTest(err) {
  if (err?.message) {
    console.error(err.message);
  }
  console.error('\nRebuild: node engine/packages/npm/build.js');
  process.exit(1);
}

export async function test() {
  console.log('Lemma package tests\n');

  try {
  if (!existsSync(join(DIST_PATH, 'lemma.js'))) {
    throw new Error('dist/ missing. Run: node engine/packages/npm/build.js');
  }

  const importRegex = /from\s+['"](\.[^'"]+)['"]/g;
  const pkgJson = JSON.parse(readFileSync(join(DIST_PATH, 'package.json'), 'utf-8'));
  const publishedFiles = pkgJson.files || [];
  for (const entry of ['lemma.js', 'lsp.js']) {
    const src = readFileSync(join(DIST_PATH, entry), 'utf-8');
    let match;
    importRegex.lastIndex = 0;
    while ((match = importRegex.exec(src)) !== null) {
      const target = join(DIST_PATH, match[1]);
      if (!existsSync(target)) {
        throw new Error(`${entry} imports '${match[1]}' but file missing`);
      }
      const rel = match[1].replace(/^\.\//, '');
      if (!publishedFiles.some((f) => rel === f || rel.startsWith(f + '/'))) {
        throw new Error(`${entry}: '${match[1]}' not in package.json files`);
      }
    }
  }
  for (const entry of publishedFiles) {
    if (!existsSync(join(DIST_PATH, entry))) {
      throw new Error(`package.json lists "${entry}" but missing in dist/`);
    }
  }
  console.log('  ok  package graph (imports + npm files)\n');

  const { initSync, Engine } = await import(join(DIST_PATH, 'lemma.js'));
  initSync({ module: readFileSync(join(DIST_PATH, 'lemma_bg.wasm')) });
  console.log('  ok  initSync + Engine\n');

  const engine = new Engine();
  let passed = 0;

  const run = async (title, fn) => {
    await case_(title, fn);
    passed++;
  };

    await run('Engine.withLimits applies named overrides', () => {
      const limited = Engine.withLimits({ max_sources: 7 });
      assert(limited.limits().max_sources === 7, 'max_sources override');
      let threw = false;
      try {
        Engine.withLimits({ not_a_limit: 1 });
      } catch {
        threw = true;
      }
      assert(threw, 'unknown limits key must throw');
    });

    await run('Engine.withLimits rejects non-integer and unsafe integers', () => {
      let threwFrac = false;
      try {
        Engine.withLimits({ max_sources: 1.5 });
      } catch {
        threwFrac = true;
      }
      assert(threwFrac, 'fractional max_sources must throw');
      let threwUnsafe = false;
      try {
        Engine.withLimits({ max_sources: 2 ** 53 });
      } catch {
        threwUnsafe = true;
      }
      assert(threwUnsafe, 'max_sources at 2**53 must throw (above MAX_SAFE_INTEGER)');
      let threwAbove = false;
      try {
        Engine.withLimits({ max_sources: 2 ** 54 });
      } catch {
        threwAbove = true;
      }
      assert(threwAbove, 'max_sources beyond f64 safe integer range must throw');
    });

    await run('Engine.snapshot round-trip restores list and run', () => {
      const source = new Engine();
      source.load(`
spec snap_demo
data x: number
rule y: x + 1
`);
      const bytes = source.snapshot();
      assert(bytes instanceof Uint8Array, 'snapshot must return Uint8Array');
      assert(bytes.length > 0, 'snapshot must be non-empty');
      const restored = Engine.fromSnapshot(bytes);
      assert(
        JSON.stringify(restored.list()) === JSON.stringify(source.list()),
        'list must match after fromSnapshot'
      );
      const runSource = source.run({ spec: 'snap_demo', data: { x: 41 } });
      const runRestored = restored.run({ spec: 'snap_demo', data: { x: 41 } });
      const dropExplanation = (rule) => {
        const { explanation: _explanation, ...rest } = rule;
        return rest;
      };
      assert(
        JSON.stringify(dropExplanation(runRestored.results.y)) ===
          JSON.stringify(dropExplanation(runSource.results.y)),
        'run rule y must match after fromSnapshot'
      );
      let threwCorrupt = false;
      try {
        const corrupt = new Uint8Array(bytes);
        corrupt[0] = 0x58;
        Engine.fromSnapshot(corrupt);
      } catch (err) {
        threwCorrupt = true;
        assert(err && typeof err === 'object', 'corrupt snapshot throws EngineError object');
      }
      assert(threwCorrupt, 'corrupt snapshot must throw');
    });

    await run('embedded lemma in list + source', () => {
      const fresh = new Engine();
      const groups = fresh.list();
      assert(
        groups.some((g) => g.repository === 'lemma'),
        `expected lemma in list: ${JSON.stringify(groups.map((g) => g.repository))}`
      );
      const repo = fresh.source('lemma', null, null);
      assert(repo.includes('spec units'), 'source text must include spec units');
      assert(
        repo.includes('trait duration'),
        'source text must include duration typedef'
      );
    });

    await run('load inline volatile source', () => {
      const fresh = new Engine();
      fresh.load(`spec inline_only
data x: 3
rule y: x + 1`);
      const r = runEx(fresh, 'inline_only', null, {}, null);
      assert(ruleNumber(r.results.y) === 4, 'inline load run');
    });

    await run('update upserts identities from code', () => {
      const fresh = new Engine();
      fresh.load({
        'pricing.lemma': `spec pricing
data quantity: 1
rule total: quantity * 10`,
      });
      fresh.update(
        null,
        `spec pricing
data quantity: 1
rule total: quantity * 20`,
        'pricing.lemma',
      );
      const r = runEx(fresh, 'pricing', null, {}, null);
      assert(ruleNumber(r.results.total) === 20, 'update must change rule result');
    });

    await run('load list of label-code pairs', () => {
      const fresh = new Engine();
      fresh.load([
        ['pair.lemma', `spec pair_test
data x: 2
rule y: x * 3`],
      ]);
      const r = runEx(fresh, 'pair_test', null, {}, null);
      assert(ruleNumber(r.results.y) === 6, 'list batch load run');
    });

    await run('load object preserves key insertion order in list', () => {
      const fresh = new Engine();
      // Non-alphabetical insertion order; HashMap would scramble this.
      fresh.load({
        'zebra.lemma': `spec zebra
data n: 1
rule r: n`,
        'alpha.lemma': `spec alpha
data n: 1
rule r: n`,
        'mike.lemma': `spec mike
data n: 1
rule r: n`,
      });
      const workspace = fresh.list().find((g) => g.repository == null);
      assert(workspace, 'workspace group must be present');
      const names = workspace.specs.map((s) => s.name);
      assert(
        JSON.stringify(names) === JSON.stringify(['zebra', 'alpha', 'mike']),
        `workspace list must follow object key order, got ${JSON.stringify(names)}`
      );
    });

    await run('load object preserves key order in parse errors', () => {
      const fresh = new Engine();
      let threw = false;
      try {
        fresh.load({
          'zebra.lemma': 'this is not lemma',
          'yankee.lemma': `spec ok
rule r: 1
`,
          'xray.lemma': 'also not lemma',
        });
      } catch (e) {
        threw = true;
        assert(Array.isArray(e), 'load throw must be array of EngineError');
        const attrs = e
          .map((err) => err.source && err.source.attribute)
          .filter(Boolean);
        assert(
          JSON.stringify(attrs) === JSON.stringify(['zebra.lemma', 'xray.lemma']),
          `parse errors must follow object key order, got ${JSON.stringify(attrs)}`
        );
      }
      assert(threw, 'load with invalid sources must throw');
    });

    await run('load + run shape + double rule', () => {
      engine.load({ 'test.lemma': `spec test
      data x: 10
      rule double: x * 2` });
      const r = runEx(engine, 'test', null, {}, null);
      assertResponseShape(r, 'test');
      assert(Object.keys(r.results).includes('double'), `keys: ${Object.keys(r.results)}`);
      assert(!r.results.double.vetoed, 'double not vetoed');
      assert(ruleNumber(r.results.double) === 20, 'double=20');
    });

    await run('list specs + show via Engine.show', () => {
      const groups = engine.list();
      assert(Array.isArray(groups) && groups.length >= 1, `list: ${JSON.stringify(groups)}`);
      const flat = flattenListGroups(groups);
      assert(flat.some((r) => r.name === 'test'), `names: ${specNames(groups)}`);
      const testRow = flat.find((r) => r.name === 'test');
      assert(!('start_line' in testRow), 'spec must not carry start_line');
      assert(!('source_type' in testRow), 'spec must not carry source_type');
      const show = engine.show(null, 'test', null);
      assert(show.spec === 'test');
      assert(typeof show.start_line === 'number' && show.start_line >= 1);
      assert(Object.keys(show.data).includes('x'));
    });

    await run('show → spec/data/rules with ShowData + ShowRule', () => {
      const show = engine.show(null, 'test', null);
      assert(show.spec === 'test');
      assert(show.data && typeof show.data === 'object');
      assert(show.rules && typeof show.rules === 'object');
      assert(Object.keys(show.data).includes('x'));
      assert(Object.keys(show.rules).includes('double'));
      const x = show.data.x;
      assert(x && typeof x === 'object' && !Array.isArray(x), 'ShowData is a named object');
      assert(x.type && typeof x.type.kind === 'string', 'type carries `kind` discriminator');
      const doubleRule = show.rules.double;
      assert(doubleRule.type && typeof doubleRule.type.kind === 'string', 'ShowRule nests type');
      assert(Array.isArray(doubleRule.branches) && doubleRule.branches.length >= 1);
      assert(Array.isArray(doubleRule.depends_on_rules));
    });

    await run('show rule result units for measure and ratio', () => {
      engine.load({ 'units_contract.lemma': `spec units_contract
data money: measure -> unit eur: 1 -> unit usd: 0.91
data rate: ratio
  -> unit basis_points: 10000
  -> unit percent: 100
  -> suggest 500 basis_points
rule total: money
rule rate_out: rate` });
      const show = engine.show(null, 'units_contract', null);
      assert(Array.isArray(show.rules.total.type.units) && show.rules.total.type.units.length >= 1);
      assert(show.rules.total.type.units[0].factor, 'measure rule units expose factor');
      assert(Array.isArray(show.rules.rate_out.type.units) && show.rules.rate_out.type.units.length >= 1);
      assert(show.rules.rate_out.type.units[0].value, 'ratio rule units expose value');
    });

    await run('run rule filter', () => {
      const r = runEx(engine, 'test', ['double'], {}, null);
      assert(Object.keys(r.results).length === 1 && r.results.double, 'filtered');
    });

    await run('run missing_data per rule; no top-level data', () => {
      engine.load({
        'missing_contract.lemma': `spec missing_contract
data n: number
rule doubled: n * 2`,
      });
      const incomplete = runEx(engine, 'missing_contract', null, {}, null);
      assertResponseShape(incomplete, 'missing_contract');
      assert(
        Array.isArray(incomplete.results.doubled.missing_data) &&
          incomplete.results.doubled.missing_data.includes('n'),
        `missing_data want n, got ${JSON.stringify(incomplete.results.doubled.missing_data)}`
      );
      const complete = runEx(engine, 'missing_contract', null, { n: 3 }, null);
      assertResponseShape(complete, 'missing_contract');
      assert(ruleNumber(complete.results.doubled) === 6, 'doubled=6');
      assert(
        !Object.prototype.hasOwnProperty.call(complete.results.doubled, 'missing_data'),
        'complete rule omits empty missing_data'
      );
    });

    await run('format()', () => {
      const out = engine.format('spec fmt\ndata a: 1\nrule r: a', null);
      assert(typeof out === 'string' && out.includes('spec fmt'));
    });

    await run('data overrides', () => {
      engine.load({ 'type_test.lemma': `spec type_test
      data number_data: 42
      data bool_data: false
      data string_data: "hello"
      data unit_data: 100
      data date_data: 2024-01-15
      rule double_number: number_data * 2` });
      const r = runEx(
        engine,
        'type_test',
        null,
        {
          number_data: 50,
          bool_data: true,
          string_data: 'world',
          unit_data: '200',
          date_data: '2024-12-25',
        },
        null
      );
      assert(ruleNumber(r.results.double_number) === 100);
    });

    await run('load parse errors as EngineError array', () => {
      let threw = false;
      try {
        engine.load({ 'bad.lemma': 'spec invalid\ndata x :' });
      } catch (e) {
        threw = true;
        assert(Array.isArray(e), 'load throw must be array of EngineError');
        assert(e.length >= 1);
        for (const err of e) assertEngineError(err);
        assert(e.some((err) => err.kind === 'parsing'), 'expected at least one parsing error');
      }
      assert(threw);
    });

    await run('load null and undefined rejected', () => {
      const fresh = new Engine();
      for (const bad of [null, undefined]) {
        let threw = false;
        try {
          fresh.load(bad);
        } catch (e) {
          threw = true;
          assert(Array.isArray(e), 'load throw must be array of EngineError');
          assert(e.length >= 1);
          for (const err of e) assertEngineError(err);
          assert(
            e.some((err) => err.kind === 'request'),
            'expected request error for null/undefined load'
          );
        }
        assert(threw, `load(${bad}) must throw`);
      }
    });

    await run('install rejects empty LemmaBase id', async () => {
      let threw = false;
      try {
        await engine.install('   ');
      } catch (e) {
        threw = true;
        assert(Array.isArray(e), 'rejection must be array of EngineError');
        assert(e.length >= 1);
        for (const err of e) assertEngineError(err);
        assert(
          e.some((err) => err.kind === 'registry' || err.kind === 'request'),
          'expected registry/request error for empty id'
        );
      }
      assert(threw);
    });

    await run('install via stubbed global fetch', async () => {
      const fixturesDir = join(__dirname, '..', '..', 'tests', 'registry_fixtures');
      const originalFetch = globalThis.fetch;
      globalThis.fetch = async (input) => {
        const url = typeof input === 'string' ? input : input.url;
        assert(
          url === 'https://lemmabase.com/@iso/countries.lemma',
          `unexpected fetch URL: ${url}`,
        );
        const content = readFileSync(join(fixturesDir, '@iso', 'countries.lemma'), 'utf-8');
        return new Response(content, { status: 200 });
      };
      try {
        const result = await engine.install('@iso/countries');
        assert(typeof result.source === 'string' && result.source.includes('alpha2'),
          'source must contain alpha2');
        assert(result.id === '@iso/countries', `id must be @iso/countries, got ${result.id}`);
      } finally {
        globalThis.fetch = originalFetch;
      }
    });

    await run('install missing id via stubbed fetch → not_found', async () => {
      const originalFetch = globalThis.fetch;
      globalThis.fetch = async (input) => {
        const url = typeof input === 'string' ? input : input.url;
        assert(
          url === 'https://lemmabase.com/@no/such-package.lemma',
          `unexpected fetch URL: ${url}`,
        );
        return new Response('not found', { status: 404 });
      };
      try {
        let threw = false;
        try {
          await engine.install('@no/such-package');
        } catch (e) {
          threw = true;
          assert(Array.isArray(e), 'rejection must be array of EngineError');
          assert(e.length >= 1);
          for (const err of e) assertEngineError(err);
          assert(
            e.some((err) => err.kind === 'registry' && err.registry_kind === 'not_found'),
            `expected registry not_found, got: ${JSON.stringify(e.map((x) => ({ kind: x.kind, registry_kind: x.registry_kind })))}`,
          );
        }
        assert(threw, 'install of missing id must throw');
      } finally {
        globalThis.fetch = originalFetch;
      }
    });

    await run('install with rejecting fetch → network_error', async () => {
      const originalFetch = globalThis.fetch;
      globalThis.fetch = async () => {
        throw new Error('connection refused');
      };
      try {
        let threw = false;
        try {
          await engine.install('@iso/countries');
        } catch (e) {
          threw = true;
          assert(Array.isArray(e), 'rejection must be array of EngineError');
          assert(e.length >= 1);
          for (const err of e) assertEngineError(err);
          assert(
            e.some((err) => err.kind === 'registry' && err.registry_kind === 'network_error'),
            `expected registry network_error, got: ${JSON.stringify(e.map((x) => ({ kind: x.kind, registry_kind: x.registry_kind })))}`,
          );
        }
        assert(threw, 'install with rejecting fetch must throw');
      } finally {
        globalThis.fetch = originalFetch;
      }
    });

    await run('invalid measure unit override completes with veto', () => {
      engine.load({ 'workspace.lemma': `spec bridge
data bridge_height: measure -> unit meter: 1.0
rule span: bridge_height` });
      const response = runEx(engine, 'bridge', null, { bridge_height: '4 mete' }, null);
      assert(response.results.span.vetoed === true, 'span must veto on unknown unit');
      assert(
        typeof response.results.span.veto_reason === 'string' &&
          response.results.span.veto_reason.includes('Unknown unit'),
        `veto_reason=${response.results.span.veto_reason}`
      );
    });

    await run('run missing spec', () => {
      let threw = false;
      try {
        runEx(engine, '__nope__', null, {}, null);
      } catch {
        threw = true;
      }
      assert(threw);
    });

    await run('data not object', () => {
      let threw = false;
      try {
        engine.run({ spec: 'test', data: 'not-an-object' });
      } catch {
        threw = true;
      }
      assert(threw);
    });

    await run('veto sqrt(-1)', () => {
      engine.load({ 'veto.lemma': `spec veto_test
      data x: 10
      rule bad_sqrt: sqrt(-1)` });
      const r = runEx(engine, 'veto_test', null, {}, null);
      assert(r.results.bad_sqrt.vetoed === true);
    });

    await run('invalid effective must error not default to now', () => {
      engine.load({ 'temporal.lemma': `spec temporal
data x: 1
rule r: x` });
      let threw = false;
      try {
        runEx(engine, 'temporal', null, {}, 'not-a-datetime');
      } catch {
        threw = true;
      }
      assert(threw, 'invalid effective string must throw before planning, not fall back to now');
    });

    await run('missing data veto', () => {
      engine.load({ 'miss.lemma': `spec missing_test
      data x: number
      data y: number
      rule sum: x + y` });
      const r = runEx(engine, 'missing_test', null, { x: 10 }, null);
      assert(r.results.sum.vetoed === true);
      assert(typeof r.results.sum.veto_reason === 'string' && r.results.sum.veto_reason.includes('y'));
    });

    await run('measure unit conversion', () => {
      // unit usd 0.84: 1 USD = 0.84 EUR (canonical). 100 usd as eur => 100 * 0.84 = 84.
      engine.load({ 'sc.lemma': `spec measure_conv
      data money: measure
        -> unit eur: 1
        -> unit usd: 0.84
      rule price_eur: 100 usd as eur` });
      const r = runEx(engine, 'measure_conv', null, {}, null);
      const eur = ruleMeasureUnit(r.results.price_eur, 'eur');
      assert(eur === 84, `expected 84 eur, got ${eur}`);
    });

    await run('cost_price measure defaults and response JSON', () => {
      engine.load({ 'cost_price.lemma': `spec cost_price
uses lemma units
data money: measure
  -> unit eur: 1.00
  -> unit inr: 0.0092
  -> decimals 2
data labor_cost: measure
  -> unit eur_per_hour: eur/hour
  -> unit inr_per_hour: inr/hour
  -> suggest 25 eur_per_hour
data product_cost: measure
  -> unit eur_per_kg: eur/kilogram
  -> unit inr_per_kg: inr/kilogram
  -> suggest 4 eur_per_kg
data throughput: measure
  -> unit kg_per_hour: kilogram/hour
  -> suggest 12 kg_per_hour
rule cost_price: product_cost + labor_cost / throughput` });
      const show = engine.show(null, 'cost_price', null);
      const laborDefault = show.data.labor_cost.suggestion;
      assert(laborDefault != null, 'labor_cost default must exist');
      assert(
        laborDefault.measure?.eur_per_hour === '25',
        `labor_cost default measure.eur_per_hour must be 25, got ${laborDefault.measure?.eur_per_hour}`
      );
      assert(
        laborDefault.measure?.inr_per_hour != null &&
          laborDefault.measure.inr_per_hour !== '25',
        `labor_cost default measure.inr_per_hour must be converted, got ${laborDefault.measure?.inr_per_hour}`
      );
      const throughputDefault = show.data.throughput.suggestion;
      assert(
        throughputDefault.measure?.kg_per_hour === '12',
        `throughput default measure.kg_per_hour must be 12, got ${throughputDefault.measure?.kg_per_hour}`
      );
      const r = runEx(
        engine,
        'cost_price',
        null,
        {
          product_cost: { eur_per_kg: '4' },
          labor_cost: { eur_per_hour: '25' },
          throughput: { kg_per_hour: '12' },
        },
        null
      );
      assertResponseShape(r, 'cost_price');
      JSON.stringify(r);
    });

    await run('ratio default JSON emits per-unit percent magnitude', () => {
      engine.load({ 'policy.lemma': `spec policy
data margin: ratio -> suggest 15%
rule m: margin` });
      const show = engine.show(null, 'policy', null);
      const marginDefault = show.data.margin.suggestion;
      assert(marginDefault != null, 'margin default must exist');
      assert(
        marginDefault.ratio?.percent === '15',
        `margin default ratio.percent must be 15, got ${marginDefault.ratio?.percent}`
      );
      const r = runEx(engine, 'policy', null, { margin: '15%' }, null);
      assertResponseShape(r, 'policy');
      JSON.stringify(r);
    });

    await run('ratio basis_points show and response JSON wire', () => {
      engine.load({ 'policy_bps.lemma': `spec policy_bps
data bps: ratio
  -> unit basis_points: 10000
  -> suggest 500 basis_points
rule m: bps` });
      const show = engine.show(null, 'policy_bps', null);
      const bpsDefault = show.data.bps.suggestion;
      assert(bpsDefault != null, 'bps default must exist');
      assert(
        bpsDefault.ratio?.basis_points === '500',
        `bps default ratio.basis_points must be 500, got ${bpsDefault.ratio?.basis_points}`
      );
      const r = runEx(
        engine,
        'policy_bps',
        null,
        { bps: { basis_points: '500' } },
        null
      );
      assertResponseShape(r, 'policy_bps');
      JSON.stringify(r);
    });

    await run('multiple specs', () => {
      engine.load({
        's1.lemma': 'spec spec1\ndata x: 1',
        's2.lemma': 'spec spec2\ndata y: 2',
      });
      assert(specNames(engine.list()).length >= 2);
    });

    await run('quality empty for clean spec', () => {
      const fresh = new Engine();
      fresh.load({
        'pricing.lemma': `spec pricing 2026-01-01
"""
Bulk pricing.
"""

data qty: number
  -> minimum 0
  -> maximum 1000000
  -> help "Order quantity."

rule total: qty
`,
      });
      const recs = fresh.quality();
      assert(Array.isArray(recs), 'quality() must return array');
      assert(recs.length === 0, `clean spec must have no recommendations, got: ${JSON.stringify(recs)}`);
    });

    await run('quality reports missing help with effective_from', () => {
      const fresh = new Engine();
      fresh.load({
        'pricing.lemma': `spec pricing 2026-01-01
"""
Bulk pricing.
"""

data qty: number
rule total: qty
`,
      });
      const recs = fresh.quality();
      assert(Array.isArray(recs) && recs.length > 0, 'must return recommendations');
      const hit = recs.find((r) => r.message && r.message.includes('no `-> help`'));
      assert(hit, `expected missing-help recommendation, got: ${JSON.stringify(recs)}`);
      assert(hit.spec === 'pricing', `spec must be pricing, got ${hit.spec}`);
      assert(
        hit.effective_from === '2026-01-01',
        `effective_from must be 2026-01-01, got ${hit.effective_from}`
      );
      assert(typeof hit.message === 'string', 'message must be string');
      assert(hit.message.includes('Consider adding a message'), `got: ${hit.message}`);
      assert(!hit.message.includes('2026'), 'message must not embed effective_from');
      assert(hit.source && typeof hit.source.attribute === 'string', 'source.attribute required');
      assert(typeof hit.source.line === 'number', 'source.line required');
    });

    await run('quality distinguishes temporal slices by effective_from', () => {
      const fresh = new Engine();
      fresh.load({
        'pricing.lemma': `spec pricing 1933-01-01
"""
Old.
"""

data qty: number
rule total: qty

spec pricing 2026-01-01
"""
New.
"""

data qty: number
rule total: qty
`,
      });
      const helps = fresh.quality().filter((r) => r.message && r.message.includes('no `-> help`'));
      assert(helps.length === 2, `expected 2 missing-help recs, got: ${JSON.stringify(helps)}`);
      assert(
        helps.some((r) => r.spec === 'pricing' && r.effective_from === '1933-01-01'),
        `missing 1933 slice: ${JSON.stringify(helps)}`
      );
      assert(
        helps.some((r) => r.spec === 'pricing' && r.effective_from === '2026-01-01'),
        `missing 2026 slice: ${JSON.stringify(helps)}`
      );
    });

    console.log(`\nAll ${passed} cases passed.`);
  } catch (err) {
    failTest(err);
  }
}

const isMain =
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href;
if (isMain) {
  await test().catch(failTest);
}
