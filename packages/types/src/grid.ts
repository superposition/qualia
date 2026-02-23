/**
 * Occupancy grid and spatial types
 */

/** Cell state in occupancy grid */
export type CellState = 'free' | 'occupied' | 'unknown';

/** Individual grid cell */
export interface GridCell {
  /** Column index */
  x: number;
  /** Row index */
  y: number;
  /** Occupancy state */
  state: CellState;
  /** Probability of occupancy [0, 1] */
  probability: number;
  /** Number of times this cell was observed */
  visitCount: number;
}

/** Occupancy grid map */
export interface OccupancyGrid {
  /** Grid width in cells */
  width: number;
  /** Grid height in cells */
  height: number;
  /** Meters per cell */
  resolution: number;
  /** World coordinates of grid origin (bottom-left) */
  origin: { x: number; y: number };
  /** Flat array of occupancy values: -1 = unknown, 0-100 = probability */
  data: number[];
}

/** Frontier between explored and unexplored space */
export interface Frontier {
  /** Frontier ID */
  id: string;
  /** Cells making up the frontier */
  cells: Array<{ x: number; y: number }>;
  /** Centroid of the frontier */
  centroid: { x: number; y: number };
  /** Number of cells in the frontier */
  size: number;
}

/** Exploration score for a frontier */
export interface ExplorationScore {
  /** Frontier being scored */
  frontier: Frontier;
  /** Distance from robot to frontier centroid */
  distance: number;
  /** Information gain estimate (proportional to size) */
  informationGain: number;
  /** Combined score (higher = better) */
  score: number;
}
