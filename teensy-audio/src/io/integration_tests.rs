//! Integration tests exercising the full I/O pipeline in software.
//!
//! These tests wire together multiple I/O components to verify end-to-end
//! data integrity without hardware. The core pattern is a software loopback:
//!
//! ```text
//! PlayQueue → OutputI2S.update() → OutputI2S.isr() → [DMA buf]
//!     → InputI2S.isr() → InputI2S.update() → RecordQueue
//! ```

#[cfg(test)]
mod tests {
    use crate::block::pool::POOL;
    use crate::block::{AudioBlockMut, AudioBlockRef};
    use crate::constants::AUDIO_BLOCK_SAMPLES;
    use crate::io::input_i2s::AudioInputI2S;
    use crate::io::output_i2s::{AudioOutputI2S, DmaHalf};
    use crate::io::play_queue::AudioPlayQueue;
    use crate::io::record_queue::AudioRecordQueue;
    use crate::node::AudioNode;

    fn reset_pool() {
        POOL.reset();
    }

    /// Generate a block filled with a simple ramp pattern, easy to verify.
    fn make_ramp(start: i16, step: i16) -> AudioBlockMut {
        let mut block = AudioBlockMut::alloc().unwrap();
        for (i, sample) in block.iter_mut().enumerate() {
            *sample = start.wrapping_add((i as i16).wrapping_mul(step));
        }
        block
    }

    /// Run one full block cycle through OutputI2S:
    /// 1. Call update() to queue the left/right blocks
    /// 2. Call isr() twice (two halves) to interleave into DMA buffer
    fn output_cycle(
        output: &mut AudioOutputI2S,
        left: Option<AudioBlockRef>,
        right: Option<AudioBlockRef>,
        dma_tx: &mut [u32; AUDIO_BLOCK_SAMPLES],
    ) -> bool {
        output.update(&[left, right], &mut []);
        let s1 = output.isr(dma_tx, DmaHalf::First);
        let s2 = output.isr(dma_tx, DmaHalf::Second);
        s1 || s2
    }

    // ---------------------------------------------------------------
    // 2.5.1: Full loopback — stereo data round-trip
    // ---------------------------------------------------------------
    #[test]
    fn full_loopback_stereo() {
        reset_pool();

        let mut play_queue = AudioPlayQueue::new();
        let mut output = AudioOutputI2S::new(true);
        let mut input = AudioInputI2S::new(false);
        let mut record_queue = AudioRecordQueue::new();
        record_queue.start();

        // Generate distinct left/right patterns
        let left_data = make_ramp(0, 1); // 0, 1, 2, 3, ...
        let right_data = make_ramp(1000, -1); // 1000, 999, 998, ...

        // Snapshot expected values before blocks are consumed
        let expected_left: [i16; AUDIO_BLOCK_SAMPLES] = core::array::from_fn(|i| {
            (0i16).wrapping_add(i as i16)
        });
        let expected_right: [i16; AUDIO_BLOCK_SAMPLES] = core::array::from_fn(|i| {
            1000i16.wrapping_add(-1 * i as i16)
        });

        // Step 1: User pushes blocks into PlayQueue
        play_queue.play(left_data).unwrap();
        play_queue.play(right_data).unwrap();

        // Step 2: PlayQueue produces blocks for the graph
        let mut pq_out_left = [None];
        let mut pq_out_right = [None];
        // Need two update calls since PlayQueue has 1 output
        play_queue.update(&[], &mut pq_out_left);
        play_queue.update(&[], &mut pq_out_right);

        let left_ref = pq_out_left[0].take().unwrap().into_shared();
        let right_ref = pq_out_right[0].take().unwrap().into_shared();

        // Step 3: Feed into OutputI2S
        let mut dma_tx = [0u32; AUDIO_BLOCK_SAMPLES];
        output_cycle(&mut output, Some(left_ref), Some(right_ref), &mut dma_tx);

        // Step 4: Simulated loopback — TX buffer becomes RX buffer
        let dma_rx = dma_tx;

        // Step 5: InputI2S needs working blocks allocated first
        let mut warmup_out = [None, None];
        input.update(&[], &mut warmup_out);
        // Now run the ISR cycle with the loopback data
        input.isr(&dma_rx, DmaHalf::First);
        input.isr(&dma_rx, DmaHalf::Second);

        // Step 6: InputI2S produces de-interleaved blocks
        let mut in_out = [None, None];
        input.update(&[], &mut in_out);
        let recv_left = in_out[0].take().expect("expected left output from input");
        let recv_right = in_out[1].take().expect("expected right output from input");

        // Step 7: Feed into RecordQueue
        let left_shared = recv_left.into_shared();
        let right_shared = recv_right.into_shared();
        record_queue.update(&[Some(left_shared)], &mut []);
        record_queue.update(&[Some(right_shared)], &mut []);

        // Step 8: Read back and verify
        let recorded_left = record_queue.read().expect("expected recorded left");
        let recorded_right = record_queue.read().expect("expected recorded right");

        for i in 0..AUDIO_BLOCK_SAMPLES {
            assert_eq!(
                recorded_left[i], expected_left[i],
                "left mismatch at sample {i}: got {}, expected {}",
                recorded_left[i], expected_left[i]
            );
            assert_eq!(
                recorded_right[i], expected_right[i],
                "right mismatch at sample {i}: got {}, expected {}",
                recorded_right[i], expected_right[i]
            );
        }
    }

    // ---------------------------------------------------------------
    // 2.5.2: Multi-block streaming
    // ---------------------------------------------------------------
    #[test]
    fn multi_block_streaming() {
        reset_pool();

        let mut play_queue = AudioPlayQueue::new();
        let mut output = AudioOutputI2S::new(true);
        let mut input = AudioInputI2S::new(false);
        let mut record_queue = AudioRecordQueue::new();
        record_queue.start();

        // Allocate working blocks for InputI2S
        let mut warmup = [None, None];
        input.update(&[], &mut warmup);

        // Stream 4 blocks, each with a distinct marker value
        for block_num in 0..4i16 {
            let marker = (block_num + 1) * 100; // 100, 200, 300, 400
            let block = make_ramp(marker, 0); // constant fill

            play_queue.play(block).unwrap();
            let mut pq_out = [None];
            play_queue.update(&[], &mut pq_out);
            let block_ref = pq_out[0].take().unwrap().into_shared();

            // Output cycle: update + 2 ISR calls
            let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];
            output_cycle(
                &mut output,
                Some(block_ref.clone()),
                Some(block_ref),
                &mut dma_buf,
            );

            // Loopback
            input.isr(&dma_buf, DmaHalf::First);
            input.isr(&dma_buf, DmaHalf::Second);

            let mut in_out = [None, None];
            input.update(&[], &mut in_out);

            if let Some(recv) = in_out[0].take() {
                let shared = recv.into_shared();
                record_queue.update(&[Some(shared)], &mut []);
            }
        }

        // Verify FIFO ordering — read blocks in order 100, 200, 300, 400
        for expected_marker in [100i16, 200, 300, 400] {
            let block = record_queue.read().expect("expected recorded block");
            assert_eq!(
                block[0], expected_marker,
                "expected marker {expected_marker}, got {}",
                block[0]
            );
        }
        assert!(record_queue.read().is_none(), "queue should be empty");
    }

    // ---------------------------------------------------------------
    // 2.5.3: Left-only and right-only (no cross-talk)
    // ---------------------------------------------------------------
    #[test]
    fn left_only_no_crosstalk() {
        reset_pool();

        let mut output = AudioOutputI2S::new(false);
        let mut input = AudioInputI2S::new(false);

        let left = make_ramp(500, 1).into_shared();

        // Feed left only, no right
        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];
        output_cycle(&mut output, Some(left), None, &mut dma_buf);

        // InputI2S: allocate working blocks, then deinterleave
        let mut warmup = [None, None];
        input.update(&[], &mut warmup);
        input.isr(&dma_buf, DmaHalf::First);
        input.isr(&dma_buf, DmaHalf::Second);
        let mut in_out = [None, None];
        input.update(&[], &mut in_out);

        let recv_left = in_out[0].take().expect("expected left");
        let recv_right = in_out[1].take().expect("expected right");

        // Left channel should have data
        for i in 0..AUDIO_BLOCK_SAMPLES {
            let expected = 500i16.wrapping_add(i as i16);
            assert_eq!(recv_left[i], expected, "left mismatch at {i}");
        }
        // Right channel should be silent
        for i in 0..AUDIO_BLOCK_SAMPLES {
            assert_eq!(recv_right[i], 0, "right should be silent at {i}, got {}", recv_right[i]);
        }
    }

    #[test]
    fn right_only_no_crosstalk() {
        reset_pool();

        let mut output = AudioOutputI2S::new(false);
        let mut input = AudioInputI2S::new(false);

        let right = make_ramp(-500, -1).into_shared();

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];
        output_cycle(&mut output, None, Some(right), &mut dma_buf);

        let mut warmup = [None, None];
        input.update(&[], &mut warmup);
        input.isr(&dma_buf, DmaHalf::First);
        input.isr(&dma_buf, DmaHalf::Second);
        let mut in_out = [None, None];
        input.update(&[], &mut in_out);

        let recv_left = in_out[0].take().expect("expected left");
        let recv_right = in_out[1].take().expect("expected right");

        // Left should be silent
        for i in 0..AUDIO_BLOCK_SAMPLES {
            assert_eq!(recv_left[i], 0, "left should be silent at {i}");
        }
        // Right should have data
        for i in 0..AUDIO_BLOCK_SAMPLES {
            let expected = (-500i16).wrapping_add((-1i16).wrapping_mul(i as i16));
            assert_eq!(recv_right[i], expected, "right mismatch at {i}");
        }
    }

    // ---------------------------------------------------------------
    // 2.5.4: Pool accounting — no block leaks
    // ---------------------------------------------------------------
    #[test]
    fn pool_accounting_no_leaks() {
        reset_pool();
        assert_eq!(POOL.allocated_count(), 0, "pool should start clean");

        {
            let mut play_queue = AudioPlayQueue::new();
            let mut output = AudioOutputI2S::new(false);
            let mut input = AudioInputI2S::new(false);
            let mut record_queue = AudioRecordQueue::new();
            record_queue.start();

            let block = make_ramp(42, 1);
            play_queue.play(block).unwrap();

            let mut pq_out = [None];
            play_queue.update(&[], &mut pq_out);
            let block_ref = pq_out[0].take().unwrap().into_shared();

            let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];
            output_cycle(
                &mut output,
                Some(block_ref.clone()),
                Some(block_ref),
                &mut dma_buf,
            );

            let mut warmup = [None, None];
            input.update(&[], &mut warmup);
            input.isr(&dma_buf, DmaHalf::First);
            input.isr(&dma_buf, DmaHalf::Second);

            let mut in_out = [None, None];
            input.update(&[], &mut in_out);

            if let Some(recv_left) = in_out[0].take() {
                let shared = recv_left.into_shared();
                record_queue.update(&[Some(shared)], &mut []);
            }
            if let Some(recv_right) = in_out[1].take() {
                let shared = recv_right.into_shared();
                record_queue.update(&[Some(shared)], &mut []);
            }

            // Drain the record queue
            while record_queue.read().is_some() {}

            // All blocks in locals will be dropped when this scope ends
        }

        assert_eq!(
            POOL.allocated_count(),
            0,
            "all blocks should be freed after pipeline drains"
        );
    }

    // ---------------------------------------------------------------
    // 2.5.5: Empty pipeline — silence
    // ---------------------------------------------------------------
    #[test]
    fn empty_pipeline_silence() {
        reset_pool();

        let mut output = AudioOutputI2S::new(true);
        let mut dma_buf = [0xDEAD_BEEFu32; AUDIO_BLOCK_SAMPLES];

        // No blocks queued — ISR should write silence
        output.isr(&mut dma_buf, DmaHalf::First);
        output.isr(&mut dma_buf, DmaHalf::Second);

        for (i, &sample) in dma_buf.iter().enumerate() {
            assert_eq!(sample, 0, "DMA buffer should be silent at index {i}, got {sample:#X}");
        }

        // InputI2S with silence buffer should produce zero blocks
        let mut input = AudioInputI2S::new(false);
        let mut warmup = [None, None];
        input.update(&[], &mut warmup);

        input.isr(&dma_buf, DmaHalf::First);
        input.isr(&dma_buf, DmaHalf::Second);

        let mut in_out = [None, None];
        input.update(&[], &mut in_out);
        let left = in_out[0].take().expect("should get left block");
        let right = in_out[1].take().expect("should get right block");

        for i in 0..AUDIO_BLOCK_SAMPLES {
            assert_eq!(left[i], 0, "left should be silent at {i}");
            assert_eq!(right[i], 0, "right should be silent at {i}");
        }
    }
}
