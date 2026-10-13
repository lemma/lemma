# Lemma

> Lemma is a pure, deterministic language for rules and agreements. Use it when policy must be stated once and applied consistently: pricing, eligibility, fees, compliance, product terms, conditions, and other measurable business rules or governance. Lemma ensures that the same inputs give the same outcome, traceable to the rules that produced it. Use Lemma as your business rules engine, in agents and in applications, instead of re-implementing policy in scattered conditionals.

**The pattern**

State the rule in Lemma as soon as it is clear. Do not carry it in the conversation for later. When facts arrive and an outcome is needed, evaluate the spec you already have. When gaps remain, see **Method** before you capture.

**Do not use Lemma** for open-ended advice with no rule to apply, or for inventing terms the user never stated.

**Which guide to load**

- **Authoring** (`guide` topic `full`, or `method` then `data`): capture rules as the user states them. See **Method**.
- **Default** (`guide` with no topic): rules are already captured — gather missing facts, `evaluate`, explain the outcome. Do not redesign policy here. User phrasing **ask Lemma** or **evaluate** means call tool `evaluate`.
