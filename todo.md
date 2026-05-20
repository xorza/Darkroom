text size wrong for damage detections

2. Cache layout::intrinsic::compute across resize ticks — independent of available_q, content unchanged → recompute is wasted (2.45% of
   resizing cycles).

4. CascadesEngine::run inner-loop annotate — largest cached hotspot (12.15%); haven't tightened since the first profile.
5. Investigate WidgetLook::animate — was below the noise floor before, now 2.48%. Likely from a recent factor-out shifting code around —
   verify it skips when no animation in flight.
