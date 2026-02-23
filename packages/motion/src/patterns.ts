/**
 * Motion pattern async generators
 *
 * Each generator yields Velocity2D commands at a configurable interval.
 * Duration parameters are in seconds. All patterns terminate when their
 * duration/distance/step count is exhausted.
 */

import type { Velocity2D } from '@qualia/types';

const DEFAULT_INTERVAL_MS = 100;

/** Helper: sleep for a given number of milliseconds */
function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/**
 * Spin in place at the given angular speed.
 * @param speed      Angular speed (rad/s)
 * @param duration   Duration in seconds (default 2)
 * @param intervalMs Yield interval in ms (default 100)
 */
export async function* spin(
  speed: number,
  duration: number = 2,
  intervalMs: number = DEFAULT_INTERVAL_MS,
): AsyncGenerator<Velocity2D> {
  const end = Date.now() + duration * 1000;
  while (Date.now() < end) {
    yield { linear: 0, angular: speed };
    await sleep(intervalMs);
  }
}

/**
 * Drive in a figure-8 pattern by alternating the angular direction.
 * Each half of the 8 lasts `segmentDuration` seconds.
 * @param linearSpeed     Forward speed (m/s)
 * @param angularSpeed    Angular speed magnitude (rad/s)
 * @param segmentDuration Duration of each half-loop in seconds (default 3)
 * @param intervalMs      Yield interval in ms (default 100)
 */
export async function* figure8(
  linearSpeed: number,
  angularSpeed: number,
  segmentDuration: number = 3,
  intervalMs: number = DEFAULT_INTERVAL_MS,
): AsyncGenerator<Velocity2D> {
  for (const direction of [1, -1]) {
    const end = Date.now() + segmentDuration * 1000;
    while (Date.now() < end) {
      yield { linear: linearSpeed, angular: angularSpeed * direction };
      await sleep(intervalMs);
    }
  }
}

/**
 * Zigzag by alternating angular direction with forward motion.
 * @param linearSpeed     Forward speed (m/s)
 * @param angularSpeed    Angular speed magnitude (rad/s)
 * @param segmentDuration Duration of each zig or zag in seconds (default 1)
 * @param intervalMs      Yield interval in ms (default 100)
 */
export async function* zigzag(
  linearSpeed: number,
  angularSpeed: number,
  segmentDuration: number = 1,
  intervalMs: number = DEFAULT_INTERVAL_MS,
): AsyncGenerator<Velocity2D> {
  for (const direction of [1, -1, 1, -1]) {
    const end = Date.now() + segmentDuration * 1000;
    while (Date.now() < end) {
      yield { linear: linearSpeed, angular: angularSpeed * direction };
      await sleep(intervalMs);
    }
  }
}

/**
 * Drive in a circle with constant linear and angular speed.
 * @param linearSpeed  Forward speed (m/s)
 * @param angularSpeed Angular speed (rad/s)
 * @param duration     Duration in seconds (default 5)
 * @param intervalMs   Yield interval in ms (default 100)
 */
export async function* circle(
  linearSpeed: number,
  angularSpeed: number,
  duration: number = 5,
  intervalMs: number = DEFAULT_INTERVAL_MS,
): AsyncGenerator<Velocity2D> {
  const end = Date.now() + duration * 1000;
  while (Date.now() < end) {
    yield { linear: linearSpeed, angular: angularSpeed };
    await sleep(intervalMs);
  }
}

/**
 * Drive in a square pattern: straight segment then 90-degree turn, four times.
 * @param linearSpeed  Forward speed (m/s) during straight segments
 * @param angularSpeed Angular speed (rad/s) during turns
 * @param sideLength   Duration of each straight segment in seconds
 * @param intervalMs   Yield interval in ms (default 100)
 */
export async function* square(
  linearSpeed: number,
  angularSpeed: number,
  sideLength: number,
  intervalMs: number = DEFAULT_INTERVAL_MS,
): AsyncGenerator<Velocity2D> {
  // Duration of a 90-degree turn at the given angular speed
  const turnDuration = (Math.PI / 2) / Math.abs(angularSpeed);

  for (let side = 0; side < 4; side++) {
    // Straight segment
    const straightEnd = Date.now() + sideLength * 1000;
    while (Date.now() < straightEnd) {
      yield { linear: linearSpeed, angular: 0 };
      await sleep(intervalMs);
    }

    // Turn segment
    const turnEnd = Date.now() + turnDuration * 1000;
    while (Date.now() < turnEnd) {
      yield { linear: 0, angular: angularSpeed };
      await sleep(intervalMs);
    }
  }
}

/**
 * Drive in an expanding spiral by gradually increasing linear speed.
 * @param startSpeed      Initial linear speed (m/s)
 * @param speedIncrement  Speed increase per step (m/s)
 * @param angularSpeed    Constant angular speed (rad/s)
 * @param steps           Number of speed increments (default 10)
 * @param intervalMs      Yield interval in ms (default 100)
 */
export async function* spiral(
  startSpeed: number,
  speedIncrement: number,
  angularSpeed: number,
  steps: number = 10,
  intervalMs: number = DEFAULT_INTERVAL_MS,
): AsyncGenerator<Velocity2D> {
  for (let i = 0; i < steps; i++) {
    const speed = startSpeed + speedIncrement * i;
    yield { linear: speed, angular: angularSpeed };
    await sleep(intervalMs);
  }
}

/**
 * Approach a point at `distance` meters away, decelerating near the target.
 * @param speed         Cruising speed (m/s)
 * @param distance      Total distance to travel (meters)
 * @param decelDistance  Distance at which to start decelerating (default distance/3)
 * @param intervalMs    Yield interval in ms (default 100)
 */
export async function* approach(
  speed: number,
  distance: number,
  decelDistance: number = distance / 3,
  intervalMs: number = DEFAULT_INTERVAL_MS,
): AsyncGenerator<Velocity2D> {
  const decel = decelDistance;
  let traveled = 0;

  while (traveled < distance) {
    const remaining = distance - traveled;
    let currentSpeed: number;

    if (remaining <= decel && decel > 0) {
      // Linear deceleration: scale speed by how much distance remains
      currentSpeed = speed * (remaining / decel);
    } else {
      currentSpeed = speed;
    }

    // Ensure minimum movement to avoid infinite loop
    currentSpeed = Math.max(currentSpeed, speed * 0.01);

    yield { linear: currentSpeed, angular: 0 };

    // Estimate distance traveled in this interval
    traveled += currentSpeed * (intervalMs / 1000);
    await sleep(intervalMs);
  }
}
