// Цвета свечения (план, разделы 3–4): циан — T1/T2, янтарь — T3, красный — ожидание подтверждения или ошибка.
// T0 (чтение) не светится.

import type { AssistantState, RiskTier } from './types';

export const PALETTE = {
  void: '#02080D',
  abyss: '#061520',
  steel: '#1B6E86',
  cyan: '#4FE3FF',
  ice: '#B8F6FF',
  amber: '#FFB547',
  red: '#FF4058',
} as const;

export type GlowKind = 'cyan' | 'amber' | 'red';

export interface GlowSpec {
  kind: GlowKind;
  color: string;
  /** Период пульсации рамки, мс: тревожные цвета пульсируют чаще. */
  pulseMs: number;
  label: string;
}

const SPECS: Record<GlowKind, GlowSpec> = {
  cyan: { kind: 'cyan', color: PALETTE.cyan, pulseMs: 2400, label: 'Действие' },
  amber: { kind: 'amber', color: PALETTE.amber, pulseMs: 1600, label: 'Опасное действие' },
  red: { kind: 'red', color: PALETTE.red, pulseMs: 900, label: 'Нужно подтверждение' },
};

export interface GlowInput {
  /** tier текущего `ControlStart` или null, если управления нет. */
  tier: RiskTier | null;
  /** Есть открытый `ConfirmRequest`. */
  confirmPending: boolean;
  assistant: AssistantState;
}

/** Цвет рамки оверлея; null — рамку не показывать. */
export function glowFor({ tier, confirmPending, assistant }: GlowInput): GlowSpec | null {
  if (confirmPending) return SPECS.red;
  if (tier === null) return null;
  if (assistant === 'error') return { ...SPECS.red, label: 'Ошибка' };
  if (tier === 'T3') return SPECS.amber;
  if (tier === 'T1' || tier === 'T2') return SPECS.cyan;
  return null; // T0
}

/** Цвет акцента реактора по состоянию ассистента. */
export function reactorAccent(state: AssistantState): string {
  switch (state) {
    case 'controlling':
      return PALETTE.amber;
    case 'error':
      return PALETTE.red;
    default:
      return PALETTE.cyan;
  }
}

/** Цвет метки tier в списках и диалогах. */
export function tierColor(tier: RiskTier): string {
  switch (tier) {
    case 'T3':
      return PALETTE.amber;
    case 'T0':
      return PALETTE.steel;
    default:
      return PALETTE.cyan;
  }
}

/** `#RRGGBB` + альфа → `rgba()`. */
export function withAlpha(hex: string, alpha: number): string {
  const h = hex.replace('#', '');
  const n = parseInt(h.length === 3 ? h.replace(/(.)/g, '$1$1') : h, 16);
  const a = Math.min(1, Math.max(0, alpha));
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${Number(a.toFixed(3))})`;
}

export const STATE_LABEL: Record<AssistantState, string> = {
  idle: 'Ожидание',
  listening: 'Слушаю',
  thinking: 'Думаю',
  speaking: 'Говорю',
  controlling: 'Управляю ПК',
  error: 'Ошибка',
};

export const TIER_LABEL: Record<RiskTier, string> = {
  T0: 'T0 · чтение',
  T1: 'T1 · действие',
  T2: 'T2 · изменения',
  T3: 'T3 · опасное',
};
