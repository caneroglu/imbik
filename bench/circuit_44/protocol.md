# 📋 Physical Breadboard Test & Measurement Protocol

Follow these sequential verification steps before connecting sensitive audio interfaces or amplifiers.

## 1. Required Bench Equipment
1. **Dual DC Power Supply** (or two series 9V batteries with center-tap GND).
2. **Digital Multimeter (DMM)** for continuity and DC operating point verification.
3. **Function / Arbitrary Waveform Generator** (capable of 100mVpp to 5Vpp sine wave at 1kHz).
4. **Dual-Channel Oscilloscope** with 10X probes (Ch1: Input, Ch2: Output).

---

## Step 1: Pre-Power Cold Resistance & Short-Circuit Inspection
> [!CAUTION]
> **DO NOT apply DC power** until completing this inspection!

1. Disconnect all external power sources.
2. Set DMM to Resistance / Continuity mode.
3. Measure resistance across rails:
   - **VCC (Node 3) to GND (Node 0)**: Must be `> 10 kΩ` (No short).
   - **VEE (Node 4) to GND (Node 0)**: Must be `> 10 kΩ` (No short).
   - **VCC (Node 3) to VEE (Node 4)**: Must be `> 20 kΩ` (No short).
4. Inspect TL072 chip orientation: Pin 1 notch must align with your breadboard layout.
5. Inspect 1N4148 diodes: Ensure the black cathode stripe matches the BOM diagram.

## Step 2: DC Power-On & Quiescent Bias Verification
1. Turn on the dual supply (+9.0V and -9.0V).
2. Observe total supply current: Normal quiescent current is **2.5 mA to 5.0 mA** per TL072 package. If current exceeds 20mA, shut off immediately and check wiring!
3. Set DMM to DC Volts mode. Ground the black lead to Node 0 (GND).
4. Measure DC voltages at key nodes:
   - **Node 3 (VCC)**: `+9.0V ± 0.3V`
   - **Node 4 (VEE)**: `-9.0V ± 0.3V`
   - **Node 2 (OUT)**: Must be between `-1.5V` and `+1.5V` (expected near 0.0V DC).
   - *If Node 2 is pinned to +7.5V or -7.5V, the op-amp is saturated to the rail. Double check negative feedback connections!*

## Step 3: Small-Signal Linear Response (100mV peak / 200mVpp, 1kHz)
1. Connect Function Generator to **Node 1 (IN)** and **Node 0 (GND)**.
2. Set waveform: **1000 Hz Sine Wave**, amplitude **200 mV peak-to-peak** (100 mV peak), 0.0V DC offset.
3. Connect Oscilloscope:
   - **Ch1 (Yellow)**: Probe Node 1 (IN).
   - **Ch2 (Blue/Green)**: Probe Node 2 (OUT).
4. Measure Ch2 peak-to-peak voltage.
5. Note linear gain: $G = V_{out,pp} / 200\text{mV}$. Verify waveform is a clean, undistorted sine wave.

## Step 4: Large-Signal Non-linear Characterization (2.0V peak / 4.0Vpp, 1kHz)
This circuit exhibits non-linear wave-shaping character!

1. Increase generator amplitude to **4.0 V peak-to-peak** (2.0 V peak), 1kHz sine.
2. Observe Ch2 output waveform on the oscilloscope:
   - **Predicted Asymmetry Index**: 0.586 (Look for positive vs negative peak clipping differences).
   - **Predicted Harmonics Ratio**: H2=0.207, H3=0.077, H5=0.048
   - **Predicted Dynamic Compression**: 0.0%
3. Check for diode knee conduction around ±0.6V or soft op-amp saturation shoulders.

## Step 5: Frequency Response & Cutoff Verification
This circuit is configured as an active filter with expected cutoff near **478 Hz**.

1. Set generator amplitude to **1.0 Vpp**.
2. Sweep generator frequency from 100 Hz up to 20 kHz.
3. Locate the -3dB frequency ($V_{out} = 0.707 \times V_{passband}$): Compare with simulated 478 Hz.
4. Verify roll-off slope in the attenuation band.

## Step 7: Oscilloscope Data Export for Ingest (Stage 9)
1. With 1kHz 4Vpp input connected, press **Single / Stop** on your oscilloscope.
2. Export the waveform to a USB flash drive as CSV format.
3. Save the file as `scope_capture.csv`.
4. Run `imbik ingest scope_capture.csv` to overlay physical reality against SPICE simulation curves!
