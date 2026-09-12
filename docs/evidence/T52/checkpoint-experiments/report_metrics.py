#!/usr/bin/env python3
"""Read T52 trainer report dumps and print the gate-relevant numbers.

Usage: report_metrics.py <report-dump.json> [...]
Non-finite metrics appear as JSON null (serde_json) and are printed as nan.
"""
import json
import math
import sys


def num(value):
    if value is None:
        return math.nan
    return float(value)


def fmt(value):
    return "nan" if math.isnan(value) else f"{value:.6g}"


def split_metrics(split):
    return {
        "samples": split["samples"],
        "sessions": split["sessions"],
        "constant": {k: num(v) for k, v in split["constant"].items()},
        "flat_mlp": {k: num(v) for k, v in split["flat_mlp"].items()},
        "tiny_cnn": {k: num(v) for k, v in split["tiny_cnn"].items()},
        "calibration": {k: num(v) for k, v in split["calibration"].items()},
        "occupancy": split["occupancy"],
    }


def predictive_pass(split):
    c, f, t = split["constant"], split["flat_mlp"], split["tiny_cnn"]
    return (
        t["transition_nll"] < c["transition_nll"]
        and t["transition_nll"] < f["transition_nll"]
        and t["rollout_error"] < c["rollout_error"]
        and t["rollout_error"] < f["rollout_error"]
    )


def calibration_pass(split):
    cal = split["calibration"]
    if cal["nonfinite_values"] != 0:
        return False
    ok = lambda v, lo, hi: lo <= v <= hi  # noqa: E731
    near = lambda v, n: abs(v - n) <= 0.05  # noqa: E731
    return (
        ok(cal["mean_standardized_squared_residual"], 0.9, 1.1)
        and ok(cal["calibration_slope"], 0.9, 1.1)
        and near(cal["coverage_50"], 0.5)
        and near(cal["coverage_90"], 0.9)
        and near(cal["coverage_95"], 0.95)
        and cal["clamp_fraction"] <= 0.01
    )


def occupancy_pass(split):
    occ = split["occupancy"]
    return occ["intersection_over_union"] > occ["trivial_iou"] and occ["pr_auc"] > occ["trivial_pr_auc"]


for path in sys.argv[1:]:
    report = json.load(open(path))
    print(f"== {path}")
    print(
        f"checkpoint_id={report['checkpoint_id']} backend={report['backend']} "
        f"epochs={report['epochs']} batch_size={report['batch_size']} seed={report['seed']} "
        f"cnn_steps={report['cnn_steps']} skipped={report['skipped_singletons']}"
    )
    rank = report["effective_rank"]
    print(
        f"effective_rank: rank={rank['effective_rank']:.6g} dims={rank['dimensions']} "
        f"samples={rank['sample_count']} trace={rank['trace']:.6g} converged={rank['converged']} "
        f"passes={rank['effective_rank'] >= 64.0 and rank['dimensions'] == 256 and rank['sample_count'] >= 4096 and rank['converged']}"
    )
    print(
        f"flags: baseline_gate_passed={report['baseline_gate_passed']} "
        f"grounding_calibration_gate_passed={report['grounding_calibration_gate_passed']} "
        f"all_gates_passed={report['all_gates_passed']}"
    )
    for name in ("validation", "test"):
        split = split_metrics(report[name])
        cal = split["calibration"]
        print(
            f"-- {name}: samples={split['samples']} sessions={split['sessions']} "
            f"predictive_ok={predictive_pass(split)} calibration_ok={calibration_pass(split)} "
            f"occupancy_ok={occupancy_pass(split)}"
        )
        print(
            f"   nll const={fmt(split['constant']['transition_nll'])} "
            f"flat={fmt(split['flat_mlp']['transition_nll'])} "
            f"cnn={fmt(split['tiny_cnn']['transition_nll'])}"
        )
        print(
            f"   rollout const={fmt(split['constant']['rollout_error'])} "
            f"flat={fmt(split['flat_mlp']['rollout_error'])} "
            f"cnn={fmt(split['tiny_cnn']['rollout_error'])}"
        )
        print(
            f"   calibration nll={fmt(cal['transition_nll'])} "
            f"mean_std_resid={fmt(cal['mean_standardized_squared_residual'])} "
            f"slope={fmt(cal['calibration_slope'])} cov50={fmt(cal['coverage_50'])} "
            f"cov90={fmt(cal['coverage_90'])} cov95={fmt(cal['coverage_95'])} "
            f"clamp={fmt(cal['clamp_fraction'])} nonfinite={cal['nonfinite_values']} passes={cal['passes']}"
        )
