use serde::{Deserialize, Serialize};

use crate::config::Autonomy;

/// Уровень риска действия (план, раздел 3).
/// T0 — чтение; T1 — действие (со свечением); T2 — изменения в профиле пользователя (с журналом отката);
/// T3 — опасное (озвучить и ждать «да, сэр»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RiskTier {
    T0,
    T1,
    T2,
    T3,
}

impl RiskTier {
    /// Показывать ли свечение «Джарвис управляет ПК».
    pub fn glows(self) -> bool {
        self >= RiskTier::T1
    }
}

/// Нужно ли подтверждение пользователя для действия этого уровня при выбранной автономии.
pub fn requires_confirmation(tier: RiskTier, autonomy: Autonomy) -> bool {
    match autonomy {
        Autonomy::AskDangerous => tier == RiskTier::T3,
        Autonomy::AskAllChanges => tier >= RiskTier::T2,
        Autonomy::Never => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_and_glow() {
        assert!(RiskTier::T3 > RiskTier::T0);
        assert!(!RiskTier::T0.glows());
        assert!(RiskTier::T1.glows());
    }

    #[test]
    fn confirmation_matrix() {
        use Autonomy::*;
        assert!(!requires_confirmation(RiskTier::T2, AskDangerous));
        assert!(requires_confirmation(RiskTier::T3, AskDangerous));
        assert!(requires_confirmation(RiskTier::T2, AskAllChanges));
        assert!(!requires_confirmation(RiskTier::T1, AskAllChanges));
        assert!(!requires_confirmation(RiskTier::T3, Never));
    }
}
