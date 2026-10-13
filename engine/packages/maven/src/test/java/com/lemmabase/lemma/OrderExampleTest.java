package com.lemmabase.lemma;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.math.BigDecimal;
import java.util.Map;
import org.junit.jupiter.api.Test;

final class OrderExampleTest {

  @Test
  void orderWithBigDecimalData() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec order
          data quantity: number
          data unit_price: number
          data tax_rate: number
          rule subtotal: quantity * unit_price
          rule tax: subtotal * tax_rate
          rule total: subtotal + tax
          """);

      Response response =
          engine.evaluate(
              EvaluateRequest.of("order")
                  .data(
                      Map.of(
                          "quantity", 3,
                          "unit_price", new BigDecimal("19.99"),
                          "tax_rate", new BigDecimal("0.21"))));

      assertEquals("order", response.spec());
      RuleResult total = response.results().get("total");
      assertNotNull(total);
      assertTrue(total instanceof RuleResult.Number);
      assertEquals(new BigDecimal("72.5637"), ((RuleResult.Number) total).number());
    }
  }
}
