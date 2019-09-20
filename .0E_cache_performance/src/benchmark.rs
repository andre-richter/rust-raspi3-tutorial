/*
 * MIT License
 *
 * Copyright (c) 2018-2019 Andre Richter <andre.o.richter@gmail.com>
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */

use crate::uart;
use core::sync::atomic::{compiler_fence, Ordering};
use cortex_a::{barrier, regs::*};

/// We assume that addr is cacheline aligned
fn batch_modify_time(addr: usize) -> Option<u64> {
    const CACHELINE_SIZE_BYTES: usize = 64; // TODO: retrieve this from a system register
    const NUM_CACHELINES_TOUCHED: usize = 5;
    const NUM_BENCH_ITERATIONS: usize = 20_000;

    const NUM_BYTES_TOUCHED: usize = CACHELINE_SIZE_BYTES * NUM_CACHELINES_TOUCHED;

    let mem = unsafe { core::slice::from_raw_parts_mut(addr as *mut usize, NUM_BYTES_TOUCHED) };

    // Benchmark starts here
    let t1 = CNTPCT_EL0.get();

    compiler_fence(Ordering::SeqCst);

    let mut temp: usize;
    for _ in 0..NUM_BENCH_ITERATIONS {
        for qword in mem.iter_mut() {
            unsafe {
                temp = core::ptr::read_volatile(qword);
                core::ptr::write_volatile(qword, temp + 1);
            }
        }
    }

    // Insert a barrier to ensure that the last memory operation has finished
    // before we retrieve the elapsed time with the subsequent counter read. Not
    // needed at all given the sample size, but let's be a bit pedantic here for
    // education purposes. For measuring single-instructions, this would be
    // needed.
    unsafe { barrier::dsb(barrier::SY) };

    let t2 = CNTPCT_EL0.get();
    let frq = u64::from(CNTFRQ_EL0.get());

    ((t2 - t1) * 1000).checked_div(frq)
}

pub fn run(uart: &uart::Uart) {
    use crate::memory::map;

    const ERROR_STRING: &str = "Something went wrong!";

    uart.puts("Benchmarking non-cacheable DRAM modifications at virtual 0x");
    uart.hex(map::virt::NON_CACHEABLE_START as u64);
    uart.puts(", physical 0x");
    uart.hex(map::virt::CACHEABLE_START as u64);
    uart.puts(":\n");

    let result_nc = match batch_modify_time(map::virt::NON_CACHEABLE_START) {
        Some(t) => {
            uart.dec(t as u32);
            uart.puts(" miliseconds.\n\n");
            t
        }
        None => {
            uart.puts(ERROR_STRING);
            return;
        }
    };

    uart.puts("Benchmarking cacheable DRAM modifications at virtual 0x");
    uart.hex(map::virt::CACHEABLE_START as u64);
    uart.puts(", physical 0x");
    uart.hex(map::virt::CACHEABLE_START as u64);
    uart.puts(":\n");

    let result_c = match batch_modify_time(map::virt::CACHEABLE_START) {
        Some(t) => {
            uart.dec(t as u32);
            uart.puts(" miliseconds.\n\n");
            t
        }
        None => {
            uart.puts(ERROR_STRING);
            return;
        }
    };

    if let Some(t) = (result_nc - result_c).checked_div(result_c) {
        uart.puts("With caching, the function is ");
        uart.dec((t * 100) as u32);
        uart.puts("% faster!\n");
    }
}
