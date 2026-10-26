//! [`SipOrder`]: a SIP polynomial's order, and the terms it holds.

/// The highest total power of a SIP polynomial's terms. Order 2 handles barrel and pincushion
/// distortion, order 3 a mustache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SipOrder {
    Two = 2,
    #[default]
    Three,
    Four,
    Five,
}

/// Every term of the highest order, `u^p·v^q` with `2 ≤ p + q ≤ 5`, by total power and then by
/// descending `p`: each order's terms are a prefix of it.
const TERMS: [(usize, usize); SipOrder::Five.term_count()] = {
    let mut terms = [(0, 0); SipOrder::Five.term_count()];
    let mut next = 0;
    let mut total = 2;
    while total <= SipOrder::Five.get() {
        let mut p = total;
        loop {
            terms[next] = (p, total - p);
            next += 1;
            if p == 0 {
                break;
            }
            p -= 1;
        }
        total += 1;
    }
    terms
};

impl SipOrder {
    /// The order as a number.
    pub const fn get(self) -> usize {
        self as usize
    }

    /// The terms of each axis's polynomial, every `u^p·v^q` with `2 ≤ p + q ≤ order`: the
    /// `(order + 1)(order + 2)/2` monomials up to `order` less the three of degree 0 and 1.
    pub const fn term_count(self) -> usize {
        let order = self.get();
        (order + 1) * (order + 2) / 2 - 3
    }

    /// The pairs a fit of this order needs: three per term, astrometry.net's practice against
    /// overfitting (order 4, 12 terms, about 36 points).
    pub const fn required_points(self) -> usize {
        3 * self.term_count()
    }

    /// The exponents `(p, q)` of each term, by total power and then by descending `p`.
    pub(crate) fn terms(self) -> &'static [(usize, usize)] {
        &TERMS[..self.term_count()]
    }
}

#[cfg(test)]
mod tests {
    use crate::registration::distortion::sip::sip_order::SipOrder;

    /// Each order holds every term up to it and no other: 3, 7, 12 and 18 of them, the last of
    /// order 5 `v⁵`, and order 3's list in full.
    #[test]
    fn each_order_holds_its_terms_by_total_power() {
        let orders = [
            SipOrder::Two,
            SipOrder::Three,
            SipOrder::Four,
            SipOrder::Five,
        ];
        assert_eq!(orders.map(SipOrder::term_count), [3, 7, 12, 18]);
        for order in orders {
            let terms = order.terms();
            assert_eq!(terms.len(), order.term_count(), "{order:?}");
            assert!(
                terms
                    .iter()
                    .all(|&(p, q)| (2..=order.get()).contains(&(p + q))),
                "{order:?}"
            );
        }
        assert_eq!(SipOrder::Five.terms().last(), Some(&(0, 5)));
        assert_eq!(
            SipOrder::Three.terms(),
            [(2, 0), (1, 1), (0, 2), (3, 0), (2, 1), (1, 2), (0, 3)]
        );
    }
}
