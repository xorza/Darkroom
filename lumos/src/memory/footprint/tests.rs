use crate::memory::footprint::Footprint;

/// A 100 B frame: decoded beside 40 B of its decoder's, 140; 30 B measured beside it leave the
/// peak there; 70 B beside it raise it to 170. Demosaiced into 300 B at a peak of 450, then
/// 200 B beside the 300: 500, of which 200 are transient.
#[test]
fn steps_chain_from_what_the_step_before_kept() {
    let decoded = Footprint {
        kept: 100,
        peak: 140,
    };
    assert_eq!(decoded.beside(30), decoded);
    let measured = decoded.beside(70);
    assert_eq!(
        measured,
        Footprint {
            kept: 100,
            peak: 170
        }
    );
    let demosaiced = measured.then(Footprint {
        kept: 300,
        peak: 450,
    });
    assert_eq!(
        demosaiced,
        Footprint {
            kept: 300,
            peak: 450
        }
    );
    let detected = demosaiced.beside(200);
    assert_eq!(
        detected,
        Footprint {
            kept: 300,
            peak: 500
        }
    );
    assert_eq!(detected.transient(), 200);
    assert_eq!(Footprint::kept(100).transient(), 0);
}
