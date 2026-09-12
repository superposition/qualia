#!/usr/bin/env python3
"""Print the promotion-gate numbers of one qualia-jepa-train report.

Usage: report_metrics.py <report.json> [...]

The trainer refuses to publish a report carrying a non-finite metric, so a
refused run has no report.json; where a run needed its numbers anyway the
report was written by the temporary dump instrument described in the README and
the file is named report-dump.json. serde_json writes a non-finite float as
null, which is printed here as nan.
"""
import json
import math
import sys

EFFECTIVE_RANK_MINIMUM = 64.0
CALIBRATION_CLAMP_FRACTION_MAX = 0.01
COVERAGE_TOLERANCE = 0.05


def num(value):
    if value is None:
        return math.nan
    return float(value)


def fmt(value):
    return "nan" if math.isnan(value) else f"{value:.6g}"


def predictive_pass(split):
    constant = split["constant"]
    flat = split["flat_mlp"]
    cnn = split["tiny_cnn"]
    return (
        cnn["transition_nll"] < constant["transition_nll"]
        and cnn["transition_nll"] < flat["transition_nll"]
        and cnn["rollout_error"] < constant["rollout_error"]
        and cnn["rollout_error"] < flat["rollout_error"]
    )


def calibration_pass(split):
    calibration = split["calibration"]
    if calibration["nonfinite_values"] != 0:
        return False
    inside = (
        0.9 <= calibration["mean_standardized_squared_residual"] <= 1.1
        and 0.9 <= calibration["calibration_slope"] <= 1.1
        and calibration["clamp_fraction"] <= CALIBRATION_CLAMP_FRACTION_MAX
    )
    for coverage, nominal in (
        ("coverage_50", 0.5),
        ("coverage_90", 0.9),
        ("coverage_95", 0.95),
    ):
        inside = inside and abs(calibration[coverage] - nominal) <= COVERAGE_TOLERANCE
    return inside


def occupancy_pass(split):
    occupancy = split["occupancy"]
    return (
        occupancy["intersection_over_union"] > occupancy["trivial_iou"]
        and occupancy["pr_auc"] > occupancy["trivial_pr_auc"]
    )


def rank_pass(report):
    rank = report["effective_rank"]
    return (
        rank["sample_count"] >= 4096
        and rank["dimensions"] == 256
        and rank["converged"]
        and rank["effective_rank"] == rank["effective_rank"]
        and rank["effective_rank"] >= EFFECTIVE_RANK_MINIMUM
    )


for path in sys.argv[1:]:
    report = json.load(open(path))
    print(f"== {path}")
    print(
        f"   epochs={report['epochs']} cnn_steps={report['cnn_steps']} "
        f"backend={report['backend']} seed={report['seed']}"
    )
    rank = report["effective_rank"]
    print(
        f"   effective_rank={fmt(num(rank['effective_rank']))} "
        f"(floor {EFFECTIVE_RANK_MINIMUM:g}, samples={rank['sample_count']}, "
        f"dims={rank['dimensions']}, trace={fmt(num(rank['trace']))}, "
        f"converged={rank['converged']}, sweeps={rank['sweeps']}) "
        f"pass={rank_pass(report)}"
    )
    for name in ("validation", "test"):
        split = report[name]
        calibration = split["calibration"]
        print(
            f"   {name}: samples={split['samples']} sessions={split['sessions']}\n"
            f"       nll      cnn={fmt(num(split['tiny_cnn']['transition_nll']))} "
            f"flat={fmt(num(split['flat_mlp']['transition_nll']))} "
            f"const={fmt(num(split['constant']['transition_nll']))}\n"
            f"       rollout  cnn={fmt(num(split['tiny_cnn']['rollout_error']))} "
            f"flat={fmt(num(split['flat_mlp']['rollout_error']))} "
            f"const={fmt(num(split['constant']['rollout_error']))}\n"
            f"       slope={fmt(num(calibration['calibration_slope']))} "
            f"msr={fmt(num(calibration['mean_standardized_squared_residual']))} "
            f"cov={fmt(num(calibration['coverage_50']))}/"
            f"{fmt(num(calibration['coverage_90']))}/"
            f"{fmt(num(calibration['coverage_95']))} "
            f"clamp={fmt(num(calibration['clamp_fraction']))} "
            f"nonfinite={calibration['nonfinite_values']}\n"
            f"       predictive={predictive_pass(split)} "
            f"calibration={calibration_pass(split)} "
            f"occupancy={occupancy_pass(split)}"
        )
    print(
        f"   report flags: baseline_gate_passed={report['baseline_gate_passed']} "
        f"grounding_calibration_gate_passed={report['grounding_calibration_gate_passed']} "
        f"all_gates_passed={report['all_gates_passed']}"
    )
