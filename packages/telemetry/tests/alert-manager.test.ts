import { describe, test, expect } from 'bun:test';
import type { Metric } from '@qualia/types';
import { AlertManager } from '../src/alert-manager';
import type { AlertRule } from '../src/alert-manager';

function makeMetric(name: string, value: number): Metric {
  return { name, value, timestamp: Date.now() };
}

function makeRule(overrides: Partial<AlertRule> = {}): AlertRule {
  return {
    name: 'test-rule',
    metricName: 'cpu',
    condition: 'gt',
    threshold: 80,
    message: 'CPU too high',
    severity: 'warning',
    ...overrides,
  };
}

describe('AlertManager', () => {
  test('addRule and getRules', () => {
    const mgr = new AlertManager();

    mgr.addRule(makeRule({ name: 'r1' }));
    mgr.addRule(makeRule({ name: 'r2' }));

    const rules = mgr.getRules();
    expect(rules).toHaveLength(2);
    expect(rules.map((r) => r.name)).toContain('r1');
    expect(rules.map((r) => r.name)).toContain('r2');
  });

  test('check triggers alert when threshold is exceeded (gt)', () => {
    const mgr = new AlertManager();
    mgr.addRule(makeRule({ condition: 'gt', threshold: 80 }));

    const alerts = mgr.check(makeMetric('cpu', 95));

    expect(alerts).toHaveLength(1);
    expect(alerts[0]!.message).toBe('CPU too high');
    expect(alerts[0]!.rule.name).toBe('test-rule');
  });

  test('check does not trigger when within threshold', () => {
    const mgr = new AlertManager();
    mgr.addRule(makeRule({ condition: 'gt', threshold: 80 }));

    const alerts = mgr.check(makeMetric('cpu', 50));

    expect(alerts).toHaveLength(0);
  });

  test('removeRule removes correctly', () => {
    const mgr = new AlertManager();
    mgr.addRule(makeRule({ name: 'to-remove' }));

    expect(mgr.getRules()).toHaveLength(1);

    const removed = mgr.removeRule('to-remove');
    expect(removed).toBe(true);
    expect(mgr.getRules()).toHaveLength(0);
  });

  test('removeRule returns false for nonexistent name', () => {
    const mgr = new AlertManager();
    expect(mgr.removeRule('nope')).toBe(false);
  });

  test('multiple rules can trigger on the same metric', () => {
    const mgr = new AlertManager();
    mgr.addRule(
      makeRule({ name: 'high', condition: 'gt', threshold: 50, severity: 'warning' }),
    );
    mgr.addRule(
      makeRule({
        name: 'critical',
        condition: 'gt',
        threshold: 90,
        severity: 'critical',
        message: 'CPU critical',
      }),
    );

    const alerts = mgr.check(makeMetric('cpu', 95));

    expect(alerts).toHaveLength(2);
    const names = alerts.map((a) => a.rule.name);
    expect(names).toContain('high');
    expect(names).toContain('critical');
  });

  test('clearHistory empties history', () => {
    const mgr = new AlertManager();
    mgr.addRule(makeRule());

    mgr.check(makeMetric('cpu', 95)); // triggers alert
    expect(mgr.getAlertHistory()).toHaveLength(1);

    mgr.clearHistory();
    expect(mgr.getAlertHistory()).toHaveLength(0);
  });

  test('getAlertHistory accumulates across checks', () => {
    const mgr = new AlertManager();
    mgr.addRule(makeRule({ condition: 'gt', threshold: 50 }));

    mgr.check(makeMetric('cpu', 60));
    mgr.check(makeMetric('cpu', 70));
    mgr.check(makeMetric('cpu', 30)); // does not trigger

    expect(mgr.getAlertHistory()).toHaveLength(2);
  });

  test('check supports lt condition', () => {
    const mgr = new AlertManager();
    mgr.addRule(
      makeRule({
        name: 'low-battery',
        metricName: 'battery',
        condition: 'lt',
        threshold: 20,
        message: 'Battery low',
      }),
    );

    const alerts = mgr.check(makeMetric('battery', 10));
    expect(alerts).toHaveLength(1);
    expect(alerts[0]!.message).toBe('Battery low');
  });

  test('check ignores rules for different metric names', () => {
    const mgr = new AlertManager();
    mgr.addRule(makeRule({ metricName: 'cpu' }));

    const alerts = mgr.check(makeMetric('memory', 95));
    expect(alerts).toHaveLength(0);
  });
});
