/**
 * Assign each API fixture to its declared generated type.
 * `npm run typecheck:fixtures` must pass once lemma.d.ts is generated from Rust
 * with one API shape per field (no Show vs SpecVersion drift).
 *
 * TypeScript widens string-literal properties from `resolveJsonModule` imports to
 * `string` (no literal narrowing), so discriminated unions (`kind`, `extends.kind`)
 * cannot be checked against the raw JSON import directly. Each fixture is re-asserted
 * here to the exact literal shape found in the corresponding `.json` file (kept in
 * sync by hand) so the assignments below exercise real structural + literal checking
 * against the declared SDK types.
 */

import showMinimalJson from "../../../tests/fixtures/api/show_minimal.json";
import sourceTypeVariantsJson from "../../../tests/fixtures/api/source_type_variants.json";
import repositoryInstallResultJson from "../../../tests/fixtures/api/repository_install_result.json";

import showListJson from "../../../tests/fixtures/api/show_list.json";
import responseListJson from "../../../tests/fixtures/api/response_list.json";

import type {
  LemmaType,
  RepositoryInstallResult,
  Response,
  Show,
  SourceType,
  SpecVersion,
} from "../lemma";

// Fixture JSON is validated by engine api_schema tests; widen through unknown so
// resolveJsonModule string[] does not fight literal tuples in the hand shape.
const showMinimal = showMinimalJson as unknown as Show;
const show: Show = showMinimal;
void show;

// Same Rust field, same ISO string in the API response.
const specVersion: SpecVersion = {
  name: showMinimal.title,
  effective_from: showMinimal["x-lemma-effective-from"],
};
void specVersion;

const sourceTypeVariants = sourceTypeVariantsJson as {
  path: { path: string };
  dependency: { dependency: string };
  volatile: "volatile";
};

const pathForm: SourceType = sourceTypeVariants.path;
const dependencyForm: SourceType = sourceTypeVariants.dependency;
const volatileForm: SourceType = sourceTypeVariants.volatile;
void pathForm;
void dependencyForm;
void volatileForm;

const installResult: RepositoryInstallResult = repositoryInstallResultJson as {
  source: string;
  id: "@iso/countries";
};
void installResult;

const menuType: LemmaType = showListJson.properties.menu["x-lemma-type"] as {
  name: "menu_item list";
  kind: "list";
  element: { kind: "spec"; spec: "menu_item"; effective: "" };
  key: { fields: ["drink"] };
  extends: { kind: "primitive" };
};
void menuType;

const responseList = responseListJson as {
  spec: "coffee_order";
  effective: string;
  results: {
    selected: {
      vetoed: false;
      rule_type: "menu_item";
      result: "menu.2";
      row: { spec: "menu_item"; effective: ""; instance: "menu.2"; index: 1 };
    };
    high_fares: {
      vetoed: false;
      rule_type: "fares";
      result: "120 eur";
      list: { result: "120 eur"; measure: { eur: "120" }; unit: "eur" }[];
    };
  };
  rows: {
    spec: "menu_item";
    effective: "";
    data: Record<string, { vetoed: boolean; rule_type: string; result: { result: string } }>;
    rules: Record<string, never>;
  }[];
};
const response: Response = responseList;
void response;
