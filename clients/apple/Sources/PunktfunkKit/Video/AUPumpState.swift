// The per-access-unit bookkeeping both VideoToolbox pumps do: the straggler filter, the format
// and decoded-size tracking, the keyframe WANT that only an IDR's parameter sets can end, and
// which post-loss AUs never reach the decoder.
//
// Pure by design — the caller reads the connection and applies what `note` returns — so the rules
// are testable without a host. It exists as one type because the two pumps carried the same
// ~60 lines and had already drifted apart: both of the loss-recovery defects found in the Apple
// client had to be fixed twice, and the second copy is easy to miss.
//
// The freeze itself (what stays on glass) is the shared `ReanchorGate`'s. This type only keeps
// reference-damaged deltas away from VideoToolbox so the gate ever sees the anchor decode.

import CoreMedia
import Foundation

struct AUPumpState {
    /// The live format description. Only an IDR's parameter sets can set it.
    private(set) var format: CMVideoFormatDescription?
    /// The last size reported upward, so a loss-recovery IDR at the same size stays quiet.
    private var lastDims: CMVideoDimensions?
    /// Newest submitted frame index, for the straggler filter.
    private var newestIndex: UInt32?
    /// Persistent WANT for the two states only parameter sets can end: no decodable format yet,
    /// or a decoder reset. The caller re-asks (throttled) while it is true.
    private(set) var awaitingIDR = false
    /// From a frame-index gap until the AU that re-anchors decode (an IDR or a flagged RFI
    /// anchor). Every delta in between references the lost picture, and one such AU puts the
    /// VideoToolbox HEVC session into an error state that refuses every later non-IDR AU — the
    /// anchor included. Withheld, the anchor decodes and lifts the gate.
    private(set) var withholding = false
    /// A recovery mark arrived while withholding: the wave builds on a chain VideoToolbox no
    /// longer has, so only an IDR ends this — keep asking.
    private var markWhileWithholding = false

    /// What the caller should do about this access unit.
    struct Step: Equatable {
        /// Arrived behind one already submitted: decoding it rewinds the reference buffer, so
        /// skip it entirely.
        var straggler = false
        /// The decoded size changed — report it (a new-mode IDR, not a same-size recovery one).
        var newSize: Size?
        /// This AU ended a recovery the pump was waiting on.
        var resumed = false
        /// The wait for a decodable format began with this AU (log once, not per AU).
        var startedFormatWait = false
        /// Do not hand this AU to the decoder: it references a lost picture (see `withholding`).
        var withhold = false
        /// Ask the host for a keyframe (the caller throttles).
        var askKeyframe = false

        struct Size: Equatable {
            var width: Int
            var height: Int
        }
    }

    /// Fold one access unit in. `idrFormat` is what the codec made of its parameter sets, or nil
    /// for a delta frame; `lossAhead` says a frame-index gap precedes this AU; `flags` are its
    /// wire flags (`AccessUnit.flags`).
    mutating func note(
        frameIndex: UInt32, idrFormat: CMVideoFormatDescription?, lossAhead: Bool = false,
        flags: UInt32 = 0
    ) -> Step {
        var step = Step()
        // Wraparound-safe: the index is a 32-bit counter, so compare the difference as signed.
        if let newest = newestIndex, Int32(bitPattern: frameIndex &- newest) <= 0 {
            step.straggler = true
            return step
        }
        newestIndex = frameIndex

        if lossAhead {
            withholding = true
            markWhileWithholding = false
        }
        if withholding {
            let reanchors =
                idrFormat != nil
                || flags & (PunktfunkConnection.flagSOF | PunktfunkConnection.userFlagRecoveryAnchor)
                    != 0
            if reanchors {
                withholding = false
            } else {
                step.withhold = true
                if flags & PunktfunkConnection.userFlagRecoveryPoint != 0 {
                    markWhileWithholding = true
                }
                step.askKeyframe = markWhileWithholding
            }
        }

        if let f = idrFormat {
            format = f // refreshed on every IDR, mode changes included
            let dims = CMVideoFormatDescriptionGetDimensions(f)
            if lastDims?.width != dims.width || lastDims?.height != dims.height {
                lastDims = dims
                step.newSize = .init(width: Int(dims.width), height: Int(dims.height))
            }
            if awaitingIDR { step.resumed = true }
            awaitingIDR = false
        }

        if format == nil {
            // Nothing decodable yet: the opening IDR's parameter sets never arrived or never
            // parsed, and under the host's infinite GOP nothing re-delivers them unless we ASK.
            // Without this every AU is dropped silently, forever.
            step.startedFormatWait = !awaitingIDR
            awaitingIDR = true
        }
        return step
    }

    /// A wedged decoder or a reset: drop the format and wait for the next parameter sets.
    mutating func requireIDR() {
        format = nil
        awaitingIDR = true
    }
}
