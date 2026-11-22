'use client';

import { fmtMb, fmtNet, fmtPercent, fmtRam, loadLevel, sparkPoints } from '@/lib/format';
import { useJarvis } from '@/lib/store';

const W = 200;
const H = 34;

function Spark({ values, max = 100 }: { values: number[]; max?: number }) {
  const pts = sparkPoints(values, W, H, max);
  return (
    <svg className="spark" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" aria-hidden="true">
      <line x1="0" y1={H - 0.5} x2={W} y2={H - 0.5} className="spark-base" />
      {pts && <polyline points={`0,${H} ${pts} ${W},${H}`} className="spark-fill" />}
      {pts && <polyline points={pts} className="spark-line" />}
    </svg>
  );
}

interface RowProps {
  name: string;
  value: string;
  detail?: string;
  pct: number | null;
  history: number[];
  max?: number;
}

function Row({ name, value, detail, pct, history, max }: RowProps) {
  return (
    <div className="tele-row" data-level={loadLevel(pct)}>
      <div className="tele-head">
        <span className="tele-name">{name}</span>
        <span className="tele-value">{value}</span>
      </div>
      {detail && <div className="tele-detail">{detail}</div>}
      <Spark values={history} max={max} />
    </div>
  );
}

/** Телеметрия ПК: CPU, память, GPU, сеть — значения и спарклайны за последнюю минуту. */
export default function Telemetry() {
  const t = useJarvis((s) => s.telemetry);
  const h = useJarvis((s) => s.telemetryHistory);
  const ramPct = t && t.ramTotalMb > 0 ? (t.ramUsedMb / t.ramTotalMb) * 100 : null;
  const netMax = Math.max(1000, ...h.net);
  return (
    <section className="panel tele" aria-label="Телеметрия">
      <h2 className="panel-title">Система</h2>
      {!t && <p className="muted small">Ждём данных от монитора ресурсов…</p>}
      <Row name="Процессор" value={fmtPercent(t?.cpu)} pct={t?.cpu ?? null} history={h.cpu} />
      <Row
        name="Память"
        value={fmtPercent(ramPct)}
        detail={t ? fmtRam(t.ramUsedMb, t.ramTotalMb) : undefined}
        pct={ramPct}
        history={h.ram}
      />
      <Row name="Видеокарта" value={t && t.gpu !== null ? fmtPercent(t.gpu) : '—'} pct={t?.gpu ?? null} history={h.gpu} />
      <Row
        name="Сеть"
        value={fmtNet(t?.netKbps)}
        detail={t && t.netKbps === null ? 'бэкенд пока не присылает' : undefined}
        pct={null}
        history={h.net}
        max={netMax}
      />
      {t && <div className="tele-foot muted small">Свободно памяти: {fmtMb(Math.max(0, t.ramTotalMb - t.ramUsedMb))}</div>}
    </section>
  );
}
