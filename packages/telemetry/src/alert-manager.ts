/**
 * AlertManager — threshold-based alerting for metrics.
 */

import type { Metric } from '@qualia/types';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type AlertSeverity = 'info' | 'warning' | 'critical';
export type AlertCondition = 'gt' | 'lt' | 'gte' | 'lte' | 'eq';

export interface AlertRule {
  /** Unique name for this rule */
  name: string;
  /** Metric name this rule applies to */
  metricName: string;
  /** Comparison condition */
  condition: AlertCondition;
  /** Threshold value */
  threshold: number;
  /** Human-readable message included in triggered alerts */
  message: string;
  /** Severity level */
  severity: AlertSeverity;
}

export interface Alert {
  /** The rule that triggered this alert */
  rule: AlertRule;
  /** The metric that caused the alert */
  metric: Metric;
  /** Unix timestamp in milliseconds when the alert was created */
  timestamp: number;
  /** Formatted alert message */
  message: string;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

export class AlertManager {
  private _rules: Map<string, AlertRule> = new Map();
  private _history: Alert[] = [];

  // -----------------------------------------------------------------------
  // Rule management
  // -----------------------------------------------------------------------

  /**
   * Register a new alert rule (or overwrite an existing rule with the same name).
   */
  addRule(rule: AlertRule): void {
    this._rules.set(rule.name, rule);
  }

  /**
   * Remove a rule by name.
   * @returns `true` if a rule was removed, `false` if no rule matched.
   */
  removeRule(name: string): boolean {
    return this._rules.delete(name);
  }

  /**
   * Return a snapshot of all registered rules.
   */
  getRules(): AlertRule[] {
    return Array.from(this._rules.values());
  }

  // -----------------------------------------------------------------------
  // Checking
  // -----------------------------------------------------------------------

  /**
   * Evaluate a metric against all registered rules whose `metricName` matches.
   * Returns every triggered alert (may be empty).
   */
  check(metric: Metric): Alert[] {
    const triggered: Alert[] = [];

    for (const rule of this._rules.values()) {
      if (rule.metricName !== metric.name) {
        continue;
      }

      if (this._evaluate(metric.value, rule.condition, rule.threshold)) {
        const alert: Alert = {
          rule,
          metric,
          timestamp: Date.now(),
          message: rule.message,
        };
        triggered.push(alert);
        this._history.push(alert);
      }
    }

    return triggered;
  }

  // -----------------------------------------------------------------------
  // History
  // -----------------------------------------------------------------------

  /**
   * Return the full alert history (oldest first).
   */
  getAlertHistory(): Alert[] {
    return [...this._history];
  }

  /**
   * Clear all stored alert history.
   */
  clearHistory(): void {
    this._history = [];
  }

  // -----------------------------------------------------------------------
  // Private helpers
  // -----------------------------------------------------------------------

  private _evaluate(
    value: number,
    condition: AlertCondition,
    threshold: number,
  ): boolean {
    switch (condition) {
      case 'gt':
        return value > threshold;
      case 'lt':
        return value < threshold;
      case 'gte':
        return value >= threshold;
      case 'lte':
        return value <= threshold;
      case 'eq':
        return value === threshold;
    }
  }
}
