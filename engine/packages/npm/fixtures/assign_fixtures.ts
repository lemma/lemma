/**
 * Assign each API fixture to its declared generated type.
 * `npm run typecheck:fixtures` must pass once lemma.d.ts is generated from Rust
 * with one API shape per field (no Show vs ListedSpec drift).
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

import type {
  ListedSpec,
  RepositoryInstallResult,
  Show,
  SourceType,
} from "../lemma";

const showMinimal = showMinimalJson as {
  spec: "sample";
  effective_from: "2024-01-01";
  versions: [{ effective_from: "2024-01-01" }];
  start_line: 1;
  source_type: "volatile";
  data: {
    amount: {
      type: {
        name: "amount";
        kind: "number";
        minimum: null;
        maximum: null;
        decimals: null;
        help: string;
        extends: {
          kind: "custom";
          parent: "number";
          family: "amount";
          defining_spec: { kind: "local" };
        };
      };
      suggestion: { number: "1" };
      needed_by_rules: ["ok"];
    };
  };
  rules: {
    ok: {
      type: {
        name: "amount";
        kind: "number";
        minimum: null;
        maximum: null;
        decimals: null;
        help: string;
        extends: {
          kind: "custom";
          parent: "number";
          family: "amount";
          defining_spec: { kind: "local" };
        };
      };
      branches: [
        {
          result: {
            type: "literal";
            number: "1";
            display: "1";
          };
        }
      ];
      depends_on_rules: [];
    };
  };
  meta: {
    title: { text: "t" };
    author: { text: "alice" };
  };
};

const show: Show = showMinimal;
void show;

// Same Rust field, same ISO string in the API response.
const listed: ListedSpec = {
  name: showMinimal.spec,
  effective_from: showMinimal.effective_from,
};
void listed;

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
