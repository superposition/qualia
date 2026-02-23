/**
 * Exploration scoring for frontier-based exploration
 *
 * Ranks frontiers by a combined score of information gain
 * (frontier size) and distance from the robot.
 */

import type { ExplorationScore, Frontier } from '@qualia/types';

export class ExplorationScorer {
  /**
   * Score and rank frontiers for exploration.
   *
   * Score = informationGain / (distance + 1)
   * where informationGain = frontier.size and
   * distance = euclidean distance from robot to frontier centroid in world coords.
   *
   * @param frontiers - Detected frontiers to score
   * @param robotX - Robot grid X coordinate
   * @param robotY - Robot grid Y coordinate
   * @param resolution - Grid resolution (meters per cell) for world-space distance
   * @returns Scored frontiers sorted by score descending (best first)
   */
  score(
    frontiers: Frontier[],
    robotX: number,
    robotY: number,
    resolution: number,
  ): ExplorationScore[] {
    const scores: ExplorationScore[] = frontiers.map((frontier) => {
      // Compute euclidean distance in world coordinates
      const dx = (frontier.centroid.x - robotX) * resolution;
      const dy = (frontier.centroid.y - robotY) * resolution;
      const distance = Math.sqrt(dx * dx + dy * dy);

      const informationGain = frontier.size;
      const score = informationGain / (distance + 1);

      return {
        frontier,
        distance,
        informationGain,
        score,
      };
    });

    // Sort by score descending (best first)
    scores.sort((a, b) => b.score - a.score);

    return scores;
  }
}
