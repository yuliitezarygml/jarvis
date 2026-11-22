//! Плавная траектория курсора: человек должен видеть движение (150–400 мс, ease-in-out, лёгкая дуга).

pub const MIN_DURATION_MS: u32 = 150;
pub const MAX_DURATION_MS: u32 = 400;
/// Шаг обновления позиции (≈100 Гц). Фактическое время сна на Windows грубее, поэтому позицию считаем по часам.
pub const STEP_MS: u32 = 10;
/// Изгиб дуги: смещение контрольной точки перпендикулярно пути, доля длины.
pub const CURVE: f64 = 0.08;
/// Изгиб не больше этого числа пикселей.
const MAX_BOW_PX: f64 = 60.0;

/// Длительность по расстоянию: короткие движения — 150 мс, длинные (≥1000 px) — 400 мс.
pub fn duration_for_distance(dist: f64) -> u32 {
    let ms = MIN_DURATION_MS as f64 + dist.max(0.0) * 0.25;
    (ms.round() as u32).clamp(MIN_DURATION_MS, MAX_DURATION_MS)
}

/// Кубический ease-in-out: медленный старт, быстрая середина, мягкая остановка.
pub fn ease_in_out_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

/// Точка траектории для доли времени `t` ∈ [0, 1]: квадратичная кривая Безье с дугой, параметр сглажен easing.
pub fn point_at(from: (i32, i32), to: (i32, i32), t: f64, curve: f64) -> (i32, i32) {
    let t = t.clamp(0.0, 1.0);
    if t >= 1.0 {
        return to;
    }
    let (x0, y0) = (from.0 as f64, from.1 as f64);
    let (x2, y2) = (to.0 as f64, to.1 as f64);
    let (dx, dy) = (x2 - x0, y2 - y0);
    let len = (dx * dx + dy * dy).sqrt();
    // Контрольная точка: середина + перпендикуляр (всегда в одну сторону — детерминированно).
    let bow = (len * curve).min(MAX_BOW_PX);
    let (nx, ny) = if len > 0.0 { (-dy / len, dx / len) } else { (0.0, 0.0) };
    let (x1, y1) = ((x0 + x2) / 2.0 + nx * bow, (y0 + y2) / 2.0 + ny * bow);
    let e = ease_in_out_cubic(t);
    let u = 1.0 - e;
    let x = u * u * x0 + 2.0 * u * e * x1 + e * e * x2;
    let y = u * u * y0 + 2.0 * u * e * y1 + e * e * y2;
    (x.round() as i32, y.round() as i32)
}

/// Дискретный план движения (для тестов и оценки): точки после старта, последняя — ровно `to`.
pub fn plan(from: (i32, i32), to: (i32, i32), duration_ms: u32, step_ms: u32, curve: f64) -> Vec<(i32, i32)> {
    let steps = (duration_ms / step_ms.max(1)).max(1);
    let mut pts: Vec<(i32, i32)> = Vec::with_capacity(steps as usize);
    for i in 1..=steps {
        let p = point_at(from, to, i as f64 / steps as f64, curve);
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    if pts.last() != Some(&to) {
        pts.push(to);
    }
    pts
}

pub fn distance(a: (i32, i32), b: (i32, i32)) -> f64 {
    let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_shape() {
        assert_eq!(ease_in_out_cubic(0.0), 0.0);
        assert_eq!(ease_in_out_cubic(1.0), 1.0);
        assert!((ease_in_out_cubic(0.5) - 0.5).abs() < 1e-12);
        // симметрия и монотонность
        for i in 0..=100 {
            let t = i as f64 / 100.0;
            assert!((ease_in_out_cubic(t) + ease_in_out_cubic(1.0 - t) - 1.0).abs() < 1e-9);
            if i > 0 {
                assert!(ease_in_out_cubic(t) >= ease_in_out_cubic(t - 0.01));
            }
        }
        // медленный старт
        assert!(ease_in_out_cubic(0.1) < 0.1);
    }

    #[test]
    fn duration_bounds() {
        assert_eq!(duration_for_distance(0.0), 150);
        assert_eq!(duration_for_distance(5000.0), 400);
        let mid = duration_for_distance(500.0);
        assert!((150..=400).contains(&mid));
        assert!(duration_for_distance(300.0) < duration_for_distance(700.0));
    }

    #[test]
    fn plan_endpoints_and_progress() {
        let from = (100, 900);
        let to = (1700, 120);
        let d = duration_for_distance(distance(from, to));
        let pts = plan(from, to, d, STEP_MS, CURVE);
        assert_eq!(*pts.last().unwrap(), to);
        assert!(pts.len() >= 10, "слишком мало шагов: {}", pts.len());
        assert!(pts.len() <= (MAX_DURATION_MS / STEP_MS) as usize + 1);
        // расстояние до цели в целом убывает (допуск на дугу/округление)
        let mut prev = distance(from, to) + 1.0;
        for p in &pts {
            let cur = distance(*p, to);
            assert!(cur <= prev + 1.5, "откат от цели: {cur} > {prev}");
            prev = cur;
        }
        // первые шаги маленькие (ease-in), середина быстрее
        let first = distance(from, pts[0]);
        let mid_i = pts.len() / 2;
        let mid = distance(pts[mid_i - 1], pts[mid_i]);
        assert!(first < mid, "нет разгона: {first} vs {mid}");
    }

    #[test]
    fn arc_is_bounded_and_point_moves_are_valid() {
        let from = (0, 0);
        let to = (2000, 0);
        let p = point_at(from, to, 0.5, CURVE);
        assert_eq!(p.0, 1000);
        assert!(p.1.abs() <= MAX_BOW_PX as i32 && p.1 != 0);
        assert_eq!(point_at(from, to, 1.0, CURVE), to);
        assert_eq!(point_at(from, to, 0.0, CURVE), from);
        // нулевое расстояние
        assert_eq!(plan((5, 5), (5, 5), 200, 10, CURVE), vec![(5, 5)]);
    }
}
