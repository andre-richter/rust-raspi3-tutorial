# Tutorial 11 - Virtual Memory Part 1: Identity Map All The Things!

## tl;dr

- The `MMU` is turned on.
- A simple scheme is used: static `64 KiB` translation tables.
- For educational purposes, we write to a remapped `UART`, and `identity map` everything else.

## Table of Contents

- [Introduction](#introduction)
- [MMU and paging theory](#mmu-and-paging-theory)
- [Approach](#approach)
  * [Generic Kernel code: `memory/mmu.rs`](#generic-kernel-code-memorymmurs)
  * [BSP: `bsp/raspberrypi/memory/mmu.rs`](#bsp-bspraspberrypimemorymmurs)
  * [AArch64: `_arch/aarch64/memory/mmu.rs`](#aarch64-_archaarch64memorymmurs)
  * [`link.ld`](#linkld)
- [Address translation examples](#address-translation-examples)
  * [Address translation using a 64 KiB page descriptor](#address-translation-using-a-64-kib-page-descriptor)
- [Zero-cost abstraction](#zero-cost-abstraction)
- [Test it](#test-it)
- [Diff to previous](#diff-to-previous)

## Introduction

Virtual memory is an immensely complex, but important and powerful topic. In this tutorial, we start
slow and easy by switching on the `MMU`, using static translation tables and `identity-map`
everything at once (except for the `UART`, which we remap for educational purposes; This will be
gone again in the next tutorial).

## MMU and paging theory

At this point, we will not re-invent the wheel and go into detailed descriptions of how paging in
modern application-grade processors works. The internet is full of great resources regarding this
topic, and we encourage you to read some of it to get a high-level understanding of the topic.

To follow the rest of this `AArch64` specific tutorial, I strongly recommend that you stop right
here and first read `Chapter 12` of the [ARM Cortex-A Series Programmer's Guide for ARMv8-A] before
you continue. This will set you up with all the `AArch64`-specific knowledge needed to follow along.

Back from reading `Chapter 12` already? Good job :+1:!

[ARM Cortex-A Series Programmer's Guide for ARMv8-A]: http://infocenter.arm.com/help/topic/com.arm.doc.den0024a/DEN0024A_v8_architecture_PG.pdf

## Approach

1. The generic `kernel` part: `src/memory/mmu.rs` provides architecture-agnostic descriptor types
   for composing a high-level data structure that describes the kernel's virtual memory layout:
   `memory::mmu::KernelVirtualLayout`.
2. The `BSP` part: `src/bsp/raspberrypi/memory/mmu.rs` contains a static instance of
   `KernelVirtualLayout` and makes it accessible through the function
   `bsp::memory::mmu::virt_mem_layout()`.
3. The `aarch64` part: `src/_arch/aarch64/memory/mmu.rs` contains the actual `MMU` driver. It picks
   up the `BSP`'s high-level `KernelVirtualLayout` and maps it using a `64 KiB` granule.

### Generic Kernel code: `memory/mmu.rs`

The descriptor types provided in this file are building blocks which help to describe attributes of
different memory regions. For example, `R/W`, `no-execute`, `cached/uncached`, and so on.

The descriptors are agnostic of the hardware `MMU`'s actual descriptors. Different `BSP`s can use
these types to produce a high-level description of the kernel's virtual memory layout. The actual
`MMU` driver for the real HW will consume these types as an input.

This way, we achieve a clean abstraction between `BSP` and `_arch` code, which allows exchanging one
without needing to adapt the other.

### BSP: `bsp/raspberrypi/memory/mmu.rs`

This file contains an instance of `KernelVirtualLayout`, which stores the descriptors mentioned
previously. The `BSP` is the correct place to do this, because it has knowledge of the target
board's memory map.

The policy is to only describe regions that are **not** ordinary, normal chacheable DRAM. However,
nothing prevents you from defining those too if you wish to. Here is an example for the device MMIO
region:

```rust
TranslationDescriptor {
    name: "Device MMIO",
    virtual_range: mmio_range_inclusive,
    physical_range_translation: Translation::Identity,
    attribute_fields: AttributeFields {
        mem_attributes: MemAttributes::Device,
        acc_perms: AccessPermissions::ReadWrite,
        execute_never: true,
    },
},
```

`KernelVirtualLayout` itself implements the following method:

```rust
pub fn virt_addr_properties(
    &self,
    virt_addr: usize,
) -> Result<(usize, AttributeFields), &'static str>
```

It will be used by the `_arch/aarch64`'s `MMU` code to request attributes for a virtual address and
the translation, which delivers the physical output address (the `usize` in the return-tuple). The
function scans for a descriptor that contains the queried address, and returns the respective
findings for the first entry that is a hit. If no entry is found, it returns default attributes for
normal chacheable DRAM and the input address, hence telling the `MMU` code that the requested
address should be `identity mapped`.

Due to this default return, it is technicall not needed to define normal cacheable DRAM regions.

### AArch64: `_arch/aarch64/memory/mmu.rs`

This file contains the `AArch64` `MMU` driver. The granule is hardcoded here (`64 KiB` page
descriptors).

The actual translation tables are stored in a global instance of the `ArchTranslationTable` struct:

```rust
/// A table descriptor for 64 KiB aperture.
///
/// The output points to the next table.
#[derive(Copy, Clone)]
#[repr(transparent)]
struct TableDescriptor(InMemoryRegister<u64, STAGE1_TABLE_DESCRIPTOR::Register>);

/// A page descriptor with 64 KiB aperture.
///
/// The output points to physical memory.
#[derive(Copy, Clone)]
#[repr(transparent)]
struct PageDescriptor(InMemoryRegister<u64, STAGE1_PAGE_DESCRIPTOR::Register>);

/// Big monolithic struct for storing the translation tables. Individual levels must be 64 KiB
/// aligned, hence the "reverse" order of appearance.
#[repr(C)]
#[repr(align(65536))]
struct FixedSizeTranslationTable<const NUM_TABLES: usize> {
    /// Page descriptors, covering 64 KiB windows per entry.
    lvl3: [[PageDescriptor; 8192]; NUM_TABLES],

    /// Table descriptors, covering 512 MiB windows.
    lvl2: [TableDescriptor; NUM_TABLES],
}

const NUM_LVL2_TABLES: usize = bsp::memory::mmu::addr_space_size() >> FIVETWELVE_MIB_SHIFT;
type ArchTranslationTable = FixedSizeTranslationTable<NUM_LVL2_TABLES>;

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

/// The translation tables.
///
/// # Safety
///
/// - Supposed to land in `.bss`. Therefore, ensure that all initial member values boil down to "0".
static mut KERNEL_TABLES: ArchTranslationTable = ArchTranslationTable::new();
```

They are populated using `bsp::memory::mmu::virt_mem_layout().virt_addr_properties()` and a bunch of
utility functions that convert our own descriptors to the actual `64 bit` integer entries needed by
the `MMU` hardware for the translation table arrays.

Each page descriptor has an entry (`AttrIndex`) that indexes into the [MAIR_EL1] register, which
holds information about the cacheability of the respective page. We currently define normal
cacheable memory and device memory (which is not cached).

[MAIR_EL1]: http://infocenter.arm.com/help/index.jsp?topic=/com.arm.doc.ddi0500d/CIHDHJBB.html

```rust
/// Setup function for the MAIR_EL1 register.
fn set_up_mair() {
    // Define the memory types being mapped.
    MAIR_EL1.write(
        // Attribute 1 - Cacheable normal DRAM.
        MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc +
        MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc +

        // Attribute 0 - Device.
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck,
    );
}
```

Afterwards, the [Translation Table Base Register 0 - EL1] is set up with the base address of the
`lvl2` tables and the [Translation Control Register - EL1] is configured.

Finally, the `MMU` is turned on through the [System Control Register - EL1]. The last step also
enables caching for data and instructions.

[Translation Table Base Register 0 - EL1]: https://docs.rs/crate/cortex-a/2.4.0/source/src/regs/ttbr0_el1.rs
[Translation Control Register - EL1]: https://docs.rs/crate/cortex-a/2.4.0/source/src/regs/tcr_el1.rs
[System Control Register - EL1]: https://docs.rs/crate/cortex-a/2.4.0/source/src/regs/sctlr_el1.rs

### `link.ld`

We need to align the `ro` section to `64 KiB` so that it doesn't overlap with the next section that
needs read/write attributes. This blows up the binary in size, but is a small price to pay
considering that it reduces the amount of static paging entries significantly, when compared to the
classical `4 KiB` granule.

## Address translation examples

For educational purposes, a layout is defined which allows to access the `UART` via two different
virtual addresses:
- Since we identity map the whole `Device MMIO` region, it is accessible by asserting its physical
  base address (`0x3F20_1000` or `0xFA20_1000` depending on which RPi you use) after the `MMU` is
  turned on.
- Additionally, it is also mapped into the last `64 KiB` slot in the first `512 MiB`, making it
  accessible through base address `0x1FFF_1000`.

The following block diagram visualizes the underlying translation for the second mapping.

### Address translation using a 64 KiB page descriptor

<img src="../doc/11_page_tables_64KiB.png" alt="Page Tables 64KiB" width="90%">

## Zero-cost abstraction

The MMU init code is again a good example to see the great potential of Rust's zero-cost
abstractions[[1]][[2]] for embedded programming.

Let's take a look again at the piece of code for setting up the `MAIR_EL1` register using the
[cortex-a] crate:

[1]: https://blog.rust-lang.org/2015/05/11/traits.html
[2]: https://ruudvanasseldonk.com/2016/11/30/zero-cost-abstractions
[cortex-a]: https://crates.io/crates/cortex-a

```rust
/// Setup function for the MAIR_EL1 register.
fn set_up_mair() {
    // Define the memory types being mapped.
    MAIR_EL1.write(
        // Attribute 1 - Cacheable normal DRAM.
        MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc +
        MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc +

        // Attribute 0 - Device.
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck,
    );
}
```

This piece of code is super expressive, and it makes use of `traits`, different `types` and
`constants` to provide type-safe register manipulation.

In the end, this code sets the first four bytes of the register to certain values according to the
data sheet. Looking at the generated code, we can see that despite all the type-safety and
abstractions, it boils down to two assembly instructions:

```text
0000000000081660 <<kernel::memory::mmu::arch_mmu::MemoryManagementUnit as kernel::memory::mmu::interface::MMU>::init>:
   ...
   816bc:       529fe088        mov     w8, #0xff04
   ...
   816c4:       d518a208        msr     mair_el1, x8
```

## Test it

```console
$ make chainboot
[...]
Minipush 1.0

[MP] ⏳ Waiting for /dev/ttyUSB0
[MP] ✅ Serial connected
[MP] 🔌 Please power the target now
 __  __ _      _ _                 _
|  \/  (_)_ _ (_) |   ___  __ _ __| |
| |\/| | | ' \| | |__/ _ \/ _` / _` |
|_|  |_|_|_||_|_|____\___/\__,_\__,_|

           Raspberry Pi 3

[ML] Requesting binary
[MP] ⏩ Pushing 64 KiB ========================================🦀 100% 32 KiB/s Time: 00:00:02
[ML] Loaded! Executing the payload now

[    3.175017] Booting on: Raspberry Pi 3
[    3.176100] MMU online. Special regions:
[    3.178009]       0x00080000 - 0x0008ffff |  64 KiB | C   RO PX  | Kernel code and RO data
[    3.182088]       0x1fff0000 - 0x1fffffff |  64 KiB | Dev RW PXN | Remapped Device MMIO
[    3.186036]       0x3f000000 - 0x4000ffff |  16 MiB | Dev RW PXN | Device MMIO
[    3.189594] Current privilege level: EL1
[    3.191502] Exception handling state:
[    3.193281]       Debug:  Masked
[    3.194843]       SError: Masked
[    3.196405]       IRQ:    Masked
[    3.197967]       FIQ:    Masked
[    3.199529] Architectural timer resolution: 52 ns
[    3.201828] Drivers loaded:
[    3.203173]       1. BCM GPIO
[    3.204605]       2. BCM PL011 UART
[    3.206297] Timer test, spinning for 1 second
[     !!!    ] Writing through the remapped UART at 0x1FFF_1000
[    4.210458] Echoing input now
```

## Diff to previous
```diff

diff -uNr 10_privilege_level/src/_arch/aarch64/memory/mmu.rs 11_virtual_mem_part1_identity_mapping/src/_arch/aarch64/memory/mmu.rs
--- 10_privilege_level/src/_arch/aarch64/memory/mmu.rs
+++ 11_virtual_mem_part1_identity_mapping/src/_arch/aarch64/memory/mmu.rs
@@ -0,0 +1,343 @@
+// SPDX-License-Identifier: MIT OR Apache-2.0
+//
+// Copyright (c) 2018-2021 Andre Richter <andre.o.richter@gmail.com>
+
+//! Memory Management Unit Driver.
+//!
+//! Static translation tables, compiled on boot; Everything 64 KiB granule.
+
+use super::{AccessPermissions, AttributeFields, MemAttributes};
+use crate::{bsp, memory};
+use core::convert;
+use cortex_a::{barrier, regs::*};
+use register::{register_bitfields, InMemoryRegister};
+
+//--------------------------------------------------------------------------------------------------
+// Private Definitions
+//--------------------------------------------------------------------------------------------------
+
+// A table descriptor, as per ARMv8-A Architecture Reference Manual Figure D5-15.
+register_bitfields! {u64,
+    STAGE1_TABLE_DESCRIPTOR [
+        /// Physical address of the next descriptor.
+        NEXT_LEVEL_TABLE_ADDR_64KiB OFFSET(16) NUMBITS(32) [], // [47:16]
+
+        TYPE  OFFSET(1) NUMBITS(1) [
+            Block = 0,
+            Table = 1
+        ],
+
+        VALID OFFSET(0) NUMBITS(1) [
+            False = 0,
+            True = 1
+        ]
+    ]
+}
+
+// A level 3 page descriptor, as per ARMv8-A Architecture Reference Manual Figure D5-17.
+register_bitfields! {u64,
+    STAGE1_PAGE_DESCRIPTOR [
+        /// Unprivileged execute-never.
+        UXN      OFFSET(54) NUMBITS(1) [
+            False = 0,
+            True = 1
+        ],
+
+        /// Privileged execute-never.
+        PXN      OFFSET(53) NUMBITS(1) [
+            False = 0,
+            True = 1
+        ],
+
+        /// Physical address of the next table descriptor (lvl2) or the page descriptor (lvl3).
+        OUTPUT_ADDR_64KiB OFFSET(16) NUMBITS(32) [], // [47:16]
+
+        /// Access flag.
+        AF       OFFSET(10) NUMBITS(1) [
+            False = 0,
+            True = 1
+        ],
+
+        /// Shareability field.
+        SH       OFFSET(8) NUMBITS(2) [
+            OuterShareable = 0b10,
+            InnerShareable = 0b11
+        ],
+
+        /// Access Permissions.
+        AP       OFFSET(6) NUMBITS(2) [
+            RW_EL1 = 0b00,
+            RW_EL1_EL0 = 0b01,
+            RO_EL1 = 0b10,
+            RO_EL1_EL0 = 0b11
+        ],
+
+        /// Memory attributes index into the MAIR_EL1 register.
+        AttrIndx OFFSET(2) NUMBITS(3) [],
+
+        TYPE     OFFSET(1) NUMBITS(1) [
+            Block = 0,
+            Table = 1
+        ],
+
+        VALID    OFFSET(0) NUMBITS(1) [
+            False = 0,
+            True = 1
+        ]
+    ]
+}
+
+const SIXTYFOUR_KIB_SHIFT: usize = 16; //  log2(64 * 1024)
+const FIVETWELVE_MIB_SHIFT: usize = 29; // log2(512 * 1024 * 1024)
+
+/// A table descriptor for 64 KiB aperture.
+///
+/// The output points to the next table.
+#[derive(Copy, Clone)]
+#[repr(transparent)]
+struct TableDescriptor(u64);
+
+/// A page descriptor with 64 KiB aperture.
+///
+/// The output points to physical memory.
+#[derive(Copy, Clone)]
+#[repr(transparent)]
+struct PageDescriptor(u64);
+
+/// Big monolithic struct for storing the translation tables. Individual levels must be 64 KiB
+/// aligned, hence the "reverse" order of appearance.
+#[repr(C)]
+#[repr(align(65536))]
+struct FixedSizeTranslationTable<const NUM_TABLES: usize> {
+    /// Page descriptors, covering 64 KiB windows per entry.
+    lvl3: [[PageDescriptor; 8192]; NUM_TABLES],
+
+    /// Table descriptors, covering 512 MiB windows.
+    lvl2: [TableDescriptor; NUM_TABLES],
+}
+
+const NUM_LVL2_TABLES: usize = bsp::memory::mmu::addr_space_size() >> FIVETWELVE_MIB_SHIFT;
+type ArchTranslationTable = FixedSizeTranslationTable<NUM_LVL2_TABLES>;
+
+trait BaseAddr {
+    fn base_addr_u64(&self) -> u64;
+    fn base_addr_usize(&self) -> usize;
+}
+
+/// Constants for indexing the MAIR_EL1.
+#[allow(dead_code)]
+mod mair {
+    pub const DEVICE: u64 = 0;
+    pub const NORMAL: u64 = 1;
+}
+
+/// Memory Management Unit type.
+struct MemoryManagementUnit;
+
+//--------------------------------------------------------------------------------------------------
+// Global instances
+//--------------------------------------------------------------------------------------------------
+
+/// The translation tables.
+///
+/// # Safety
+///
+/// - Supposed to land in `.bss`. Therefore, ensure that all initial member values boil down to "0".
+static mut KERNEL_TABLES: ArchTranslationTable = ArchTranslationTable::new();
+
+static MMU: MemoryManagementUnit = MemoryManagementUnit;
+
+//--------------------------------------------------------------------------------------------------
+// Private Code
+//--------------------------------------------------------------------------------------------------
+
+impl<T, const N: usize> BaseAddr for [T; N] {
+    fn base_addr_u64(&self) -> u64 {
+        self as *const T as u64
+    }
+
+    fn base_addr_usize(&self) -> usize {
+        self as *const _ as usize
+    }
+}
+
+impl convert::From<usize> for TableDescriptor {
+    fn from(next_lvl_table_addr: usize) -> Self {
+        let val = InMemoryRegister::<u64, STAGE1_TABLE_DESCRIPTOR::Register>::new(0);
+
+        let shifted = next_lvl_table_addr >> SIXTYFOUR_KIB_SHIFT;
+        val.write(
+            STAGE1_TABLE_DESCRIPTOR::VALID::True
+                + STAGE1_TABLE_DESCRIPTOR::TYPE::Table
+                + STAGE1_TABLE_DESCRIPTOR::NEXT_LEVEL_TABLE_ADDR_64KiB.val(shifted as u64),
+        );
+
+        TableDescriptor(val.get())
+    }
+}
+
+/// Convert the kernel's generic memory attributes to HW-specific attributes of the MMU.
+impl convert::From<AttributeFields>
+    for register::FieldValue<u64, STAGE1_PAGE_DESCRIPTOR::Register>
+{
+    fn from(attribute_fields: AttributeFields) -> Self {
+        // Memory attributes.
+        let mut desc = match attribute_fields.mem_attributes {
+            MemAttributes::CacheableDRAM => {
+                STAGE1_PAGE_DESCRIPTOR::SH::InnerShareable
+                    + STAGE1_PAGE_DESCRIPTOR::AttrIndx.val(mair::NORMAL)
+            }
+            MemAttributes::Device => {
+                STAGE1_PAGE_DESCRIPTOR::SH::OuterShareable
+                    + STAGE1_PAGE_DESCRIPTOR::AttrIndx.val(mair::DEVICE)
+            }
+        };
+
+        // Access Permissions.
+        desc += match attribute_fields.acc_perms {
+            AccessPermissions::ReadOnly => STAGE1_PAGE_DESCRIPTOR::AP::RO_EL1,
+            AccessPermissions::ReadWrite => STAGE1_PAGE_DESCRIPTOR::AP::RW_EL1,
+        };
+
+        // The execute-never attribute is mapped to PXN in AArch64.
+        desc += if attribute_fields.execute_never {
+            STAGE1_PAGE_DESCRIPTOR::PXN::True
+        } else {
+            STAGE1_PAGE_DESCRIPTOR::PXN::False
+        };
+
+        // Always set unprivileged exectue-never as long as userspace is not implemented yet.
+        desc += STAGE1_PAGE_DESCRIPTOR::UXN::True;
+
+        desc
+    }
+}
+
+impl PageDescriptor {
+    /// Create an instance.
+    fn new(output_addr: usize, attribute_fields: AttributeFields) -> Self {
+        let val = InMemoryRegister::<u64, STAGE1_PAGE_DESCRIPTOR::Register>::new(0);
+
+        let shifted = output_addr as u64 >> SIXTYFOUR_KIB_SHIFT;
+        val.write(
+            STAGE1_PAGE_DESCRIPTOR::VALID::True
+                + STAGE1_PAGE_DESCRIPTOR::AF::True
+                + attribute_fields.into()
+                + STAGE1_PAGE_DESCRIPTOR::TYPE::Table
+                + STAGE1_PAGE_DESCRIPTOR::OUTPUT_ADDR_64KiB.val(shifted),
+        );
+
+        Self(val.get())
+    }
+}
+
+impl<const NUM_TABLES: usize> FixedSizeTranslationTable<{ NUM_TABLES }> {
+    /// Create an instance.
+    pub const fn new() -> Self {
+        assert!(NUM_TABLES > 0);
+
+        Self {
+            lvl3: [[PageDescriptor(0); 8192]; NUM_TABLES],
+            lvl2: [TableDescriptor(0); NUM_TABLES],
+        }
+    }
+}
+
+/// Setup function for the MAIR_EL1 register.
+fn set_up_mair() {
+    // Define the memory types being mapped.
+    MAIR_EL1.write(
+        // Attribute 1 - Cacheable normal DRAM.
+        MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc +
+        MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc +
+
+        // Attribute 0 - Device.
+        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck,
+    );
+}
+
+/// Iterates over all static translation table entries and fills them at once.
+///
+/// # Safety
+///
+/// - Modifies a `static mut`. Ensure it only happens from here.
+unsafe fn populate_tt_entries() -> Result<(), &'static str> {
+    for (l2_nr, l2_entry) in KERNEL_TABLES.lvl2.iter_mut().enumerate() {
+        *l2_entry = KERNEL_TABLES.lvl3[l2_nr].base_addr_usize().into();
+
+        for (l3_nr, l3_entry) in KERNEL_TABLES.lvl3[l2_nr].iter_mut().enumerate() {
+            let virt_addr = (l2_nr << FIVETWELVE_MIB_SHIFT) + (l3_nr << SIXTYFOUR_KIB_SHIFT);
+
+            let (output_addr, attribute_fields) =
+                bsp::memory::mmu::virt_mem_layout().virt_addr_properties(virt_addr)?;
+
+            *l3_entry = PageDescriptor::new(output_addr, attribute_fields);
+        }
+    }
+
+    Ok(())
+}
+
+/// Configure various settings of stage 1 of the EL1 translation regime.
+fn configure_translation_control() {
+    let ips = ID_AA64MMFR0_EL1.read(ID_AA64MMFR0_EL1::PARange);
+    let t0sz: u64 = bsp::memory::mmu::addr_space_size().trailing_zeros().into();
+
+    TCR_EL1.write(
+        TCR_EL1::TBI0::Ignored
+            + TCR_EL1::IPS.val(ips)
+            + TCR_EL1::EPD1::DisableTTBR1Walks
+            + TCR_EL1::TG0::KiB_64
+            + TCR_EL1::SH0::Inner
+            + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
+            + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
+            + TCR_EL1::EPD0::EnableTTBR0Walks
+            + TCR_EL1::T0SZ.val(t0sz),
+    );
+}
+
+//--------------------------------------------------------------------------------------------------
+// Public Code
+//--------------------------------------------------------------------------------------------------
+
+/// Return a reference to the MMU instance.
+pub fn mmu() -> &'static impl memory::mmu::interface::MMU {
+    &MMU
+}
+
+//------------------------------------------------------------------------------
+// OS Interface Code
+//------------------------------------------------------------------------------
+
+impl memory::mmu::interface::MMU for MemoryManagementUnit {
+    unsafe fn init(&self) -> Result<(), &'static str> {
+        // Fail early if translation granule is not supported. Both RPis support it, though.
+        if !ID_AA64MMFR0_EL1.matches_all(ID_AA64MMFR0_EL1::TGran64::Supported) {
+            return Err("Translation granule not supported in HW");
+        }
+
+        // Prepare the memory attribute indirection register.
+        set_up_mair();
+
+        // Populate translation tables.
+        populate_tt_entries()?;
+
+        // Set the "Translation Table Base Register".
+        TTBR0_EL1.set_baddr(KERNEL_TABLES.lvl2.base_addr_u64());
+
+        configure_translation_control();
+
+        // Switch the MMU on.
+        //
+        // First, force all previous changes to be seen before the MMU is enabled.
+        barrier::isb(barrier::SY);
+
+        // Enable the MMU and turn on data and instruction caching.
+        SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);
+
+        // Force MMU init to complete before next instruction.
+        barrier::isb(barrier::SY);
+
+        Ok(())
+    }
+}

diff -uNr 10_privilege_level/src/bsp/raspberrypi/link.ld 11_virtual_mem_part1_identity_mapping/src/bsp/raspberrypi/link.ld
--- 10_privilege_level/src/bsp/raspberrypi/link.ld
+++ 11_virtual_mem_part1_identity_mapping/src/bsp/raspberrypi/link.ld
@@ -8,6 +8,7 @@
     /* Set current address to the value from which the RPi starts execution */
     . = 0x80000;

+    __ro_start = .;
     .text :
     {
         *(.text._start) *(.text*)
@@ -17,6 +18,8 @@
     {
         *(.rodata*)
     }
+    . = ALIGN(65536); /* Fill up to 64 KiB */
+    __ro_end = .;

     .data :
     {

diff -uNr 10_privilege_level/src/bsp/raspberrypi/memory/mmu.rs 11_virtual_mem_part1_identity_mapping/src/bsp/raspberrypi/memory/mmu.rs
--- 10_privilege_level/src/bsp/raspberrypi/memory/mmu.rs
+++ 11_virtual_mem_part1_identity_mapping/src/bsp/raspberrypi/memory/mmu.rs
@@ -0,0 +1,93 @@
+// SPDX-License-Identifier: MIT OR Apache-2.0
+//
+// Copyright (c) 2018-2021 Andre Richter <andre.o.richter@gmail.com>
+
+//! BSP Memory Management Unit.
+
+use super::map as memory_map;
+use crate::memory::mmu::*;
+use core::ops::RangeInclusive;
+
+//--------------------------------------------------------------------------------------------------
+// Public Definitions
+//--------------------------------------------------------------------------------------------------
+
+const NUM_MEM_RANGES: usize = 3;
+
+/// The virtual memory layout.
+///
+/// The layout must contain only special ranges, aka anything that is _not_ normal cacheable DRAM.
+/// It is agnostic of the paging granularity that the architecture's MMU will use.
+pub static LAYOUT: KernelVirtualLayout<{ NUM_MEM_RANGES }> = KernelVirtualLayout::new(
+    memory_map::END_INCLUSIVE,
+    [
+        TranslationDescriptor {
+            name: "Kernel code and RO data",
+            virtual_range: ro_range_inclusive,
+            physical_range_translation: Translation::Identity,
+            attribute_fields: AttributeFields {
+                mem_attributes: MemAttributes::CacheableDRAM,
+                acc_perms: AccessPermissions::ReadOnly,
+                execute_never: false,
+            },
+        },
+        TranslationDescriptor {
+            name: "Remapped Device MMIO",
+            virtual_range: remapped_mmio_range_inclusive,
+            physical_range_translation: Translation::Offset(memory_map::mmio::START + 0x20_0000),
+            attribute_fields: AttributeFields {
+                mem_attributes: MemAttributes::Device,
+                acc_perms: AccessPermissions::ReadWrite,
+                execute_never: true,
+            },
+        },
+        TranslationDescriptor {
+            name: "Device MMIO",
+            virtual_range: mmio_range_inclusive,
+            physical_range_translation: Translation::Identity,
+            attribute_fields: AttributeFields {
+                mem_attributes: MemAttributes::Device,
+                acc_perms: AccessPermissions::ReadWrite,
+                execute_never: true,
+            },
+        },
+    ],
+);
+
+//--------------------------------------------------------------------------------------------------
+// Private Code
+//--------------------------------------------------------------------------------------------------
+
+fn ro_range_inclusive() -> RangeInclusive<usize> {
+    // Notice the subtraction to turn the exclusive end into an inclusive end.
+    #[allow(clippy::range_minus_one)]
+    RangeInclusive::new(super::ro_start(), super::ro_end() - 1)
+}
+
+fn remapped_mmio_range_inclusive() -> RangeInclusive<usize> {
+    // The last 64 KiB slot in the first 512 MiB
+    RangeInclusive::new(0x1FFF_0000, 0x1FFF_FFFF)
+}
+
+fn mmio_range_inclusive() -> RangeInclusive<usize> {
+    RangeInclusive::new(memory_map::mmio::START, memory_map::mmio::END_INCLUSIVE)
+}
+
+//--------------------------------------------------------------------------------------------------
+// Public Code
+//--------------------------------------------------------------------------------------------------
+
+/// Return the address space size in bytes.
+///
+/// Guarantees size to be a power of two.
+pub const fn addr_space_size() -> usize {
+    let size = memory_map::END_INCLUSIVE + 1;
+    assert!(size.is_power_of_two());
+
+    size
+}
+
+/// Return a reference to the virtual memory layout.
+pub fn virt_mem_layout() -> &'static KernelVirtualLayout<{ NUM_MEM_RANGES }> {
+    &LAYOUT
+}

diff -uNr 10_privilege_level/src/bsp/raspberrypi/memory.rs 11_virtual_mem_part1_identity_mapping/src/bsp/raspberrypi/memory.rs
--- 10_privilege_level/src/bsp/raspberrypi/memory.rs
+++ 11_virtual_mem_part1_identity_mapping/src/bsp/raspberrypi/memory.rs
@@ -4,6 +4,8 @@

 //! BSP Memory Management.

+pub mod mmu;
+
 use core::{cell::UnsafeCell, ops::RangeInclusive};

 //--------------------------------------------------------------------------------------------------
@@ -14,6 +16,8 @@
 extern "Rust" {
     static __bss_start: UnsafeCell<u64>;
     static __bss_end_inclusive: UnsafeCell<u64>;
+    static __ro_start: UnsafeCell<()>;
+    static __ro_end: UnsafeCell<()>;
 }

 //--------------------------------------------------------------------------------------------------
@@ -23,6 +27,21 @@
 /// The board's memory map.
 #[rustfmt::skip]
 pub(super) mod map {
+    /// The inclusive end address of the memory map.
+    ///
+    /// End address + 1 must be power of two.
+    ///
+    /// # Note
+    ///
+    /// RPi3 and RPi4 boards can have different amounts of RAM. To make our code lean for
+    /// educational purposes, we set the max size of the address space to 4 GiB regardless of board.
+    /// This way, we can map the entire range that we need (end of MMIO for RPi4) in one take.
+    ///
+    /// However, making this trade-off has the downside of making it possible for the CPU to assert a
+    /// physical address that is not backed by any DRAM (e.g. accessing an address close to 4 GiB on
+    /// an RPi3 that comes with 1 GiB of RAM). This would result in a crash or other kind of error.
+    pub const END_INCLUSIVE:       usize = 0xFFFF_FFFF;
+
     pub const BOOT_CORE_STACK_END: usize = 0x8_0000;

     pub const GPIO_OFFSET:         usize = 0x0020_0000;
@@ -36,6 +55,7 @@
         pub const START:            usize =         0x3F00_0000;
         pub const GPIO_START:       usize = START + GPIO_OFFSET;
         pub const PL011_UART_START: usize = START + UART_OFFSET;
+        pub const END_INCLUSIVE:    usize =         0x4000_FFFF;
     }

     /// Physical devices.
@@ -46,10 +66,35 @@
         pub const START:            usize =         0xFE00_0000;
         pub const GPIO_START:       usize = START + GPIO_OFFSET;
         pub const PL011_UART_START: usize = START + UART_OFFSET;
+        pub const END_INCLUSIVE:    usize =         0xFF84_FFFF;
     }
 }

 //--------------------------------------------------------------------------------------------------
+// Private Code
+//--------------------------------------------------------------------------------------------------
+
+/// Start address of the Read-Only (RO) range.
+///
+/// # Safety
+///
+/// - Value is provided by the linker script and must be trusted as-is.
+#[inline(always)]
+fn ro_start() -> usize {
+    unsafe { __ro_start.get() as usize }
+}
+
+/// Size of the Read-Only (RO) range of the kernel binary.
+///
+/// # Safety
+///
+/// - Value is provided by the linker script and must be trusted as-is.
+#[inline(always)]
+fn ro_end() -> usize {
+    unsafe { __ro_end.get() as usize }
+}
+
+//--------------------------------------------------------------------------------------------------
 // Public Code
 //--------------------------------------------------------------------------------------------------


diff -uNr 10_privilege_level/src/bsp.rs 11_virtual_mem_part1_identity_mapping/src/bsp.rs
--- 10_privilege_level/src/bsp.rs
+++ 11_virtual_mem_part1_identity_mapping/src/bsp.rs
@@ -4,7 +4,7 @@

 //! Conditional re-exporting of Board Support Packages.

-mod device_driver;
+pub mod device_driver;

 #[cfg(any(feature = "bsp_rpi3", feature = "bsp_rpi4"))]
 mod raspberrypi;

diff -uNr 10_privilege_level/src/main.rs 11_virtual_mem_part1_identity_mapping/src/main.rs
--- 10_privilege_level/src/main.rs
+++ 11_virtual_mem_part1_identity_mapping/src/main.rs
@@ -11,10 +11,12 @@
 //!
 //! - [`bsp::console::console()`] - Returns a reference to the kernel's [console interface].
 //! - [`bsp::driver::driver_manager()`] - Returns a reference to the kernel's [driver interface].
+//! - [`memory::mmu::mmu()`] - Returns a reference to the kernel's [MMU interface].
 //! - [`time::time_manager()`] - Returns a reference to the kernel's [timer interface].
 //!
 //! [console interface]: ../libkernel/console/interface/index.html
 //! [driver interface]: ../libkernel/driver/interface/trait.DriverManager.html
+//! [MMU interface]: ../libkernel/memory/mmu/interface/trait.MMU.html
 //! [timer interface]: ../libkernel/time/interface/trait.TimeManager.html
 //!
 //! # Code organization and architecture
@@ -102,7 +104,10 @@
 //! - `crate::memory::*`
 //! - `crate::bsp::memory::*`

+#![allow(incomplete_features)]
 #![feature(const_fn_fn_ptr_basics)]
+#![feature(const_generics)]
+#![feature(const_panic)]
 #![feature(format_args_nl)]
 #![feature(panic_info_message)]
 #![feature(trait_alias)]
@@ -129,9 +134,18 @@
 /// # Safety
 ///
 /// - Only a single core must be active and running this function.
-/// - The init calls in this function must appear in the correct order.
+/// - The init calls in this function must appear in the correct order:
+///     - Virtual memory must be activated before the device drivers.
+///       - Without it, any atomic operations, e.g. the yet-to-be-introduced spinlocks in the device
+///         drivers (which currently employ NullLocks instead of spinlocks), will fail to work on
+///         the RPi SoCs.
 unsafe fn kernel_init() -> ! {
     use driver::interface::DriverManager;
+    use memory::mmu::interface::MMU;
+
+    if let Err(string) = memory::mmu::mmu().init() {
+        panic!("MMU: {}", string);
+    }

     for i in bsp::driver::driver_manager().all_device_drivers().iter() {
         if let Err(x) = i.init() {
@@ -155,6 +169,9 @@

     info!("Booting on: {}", bsp::board_name());

+    info!("MMU online. Special regions:");
+    bsp::memory::mmu::virt_mem_layout().print_layout();
+
     let (_, privilege_level) = exception::current_privilege_level();
     info!("Current privilege level: {}", privilege_level);

@@ -178,6 +195,13 @@
     info!("Timer test, spinning for 1 second");
     time::time_manager().spin_for(Duration::from_secs(1));

+    let remapped_uart = unsafe { bsp::device_driver::PL011Uart::new(0x1FFF_1000) };
+    writeln!(
+        remapped_uart,
+        "[     !!!    ] Writing through the remapped UART at 0x1FFF_1000"
+    )
+    .unwrap();
+
     info!("Echoing input now");

     // Discard any spurious received characters before going into echo mode.

diff -uNr 10_privilege_level/src/memory/mmu.rs 11_virtual_mem_part1_identity_mapping/src/memory/mmu.rs
--- 10_privilege_level/src/memory/mmu.rs
+++ 11_virtual_mem_part1_identity_mapping/src/memory/mmu.rs
@@ -0,0 +1,199 @@
+// SPDX-License-Identifier: MIT OR Apache-2.0
+//
+// Copyright (c) 2020-2021 Andre Richter <andre.o.richter@gmail.com>
+
+//! Memory Management Unit.
+//!
+//! In order to decouple `BSP` and `arch` parts of the MMU code (to keep them pluggable), this file
+//! provides types for composing an architecture-agnostic description of the kernel 's virtual
+//! memory layout.
+//!
+//! The `BSP` provides such a description through the `bsp::memory::mmu::virt_mem_layout()`
+//! function.
+//!
+//! The `MMU` driver of the `arch` code uses `bsp::memory::mmu::virt_mem_layout()` to compile and
+//! install respective translation tables.
+
+#[cfg(target_arch = "aarch64")]
+#[path = "../_arch/aarch64/memory/mmu.rs"]
+mod arch_mmu;
+pub use arch_mmu::*;
+
+use core::{fmt, ops::RangeInclusive};
+
+//--------------------------------------------------------------------------------------------------
+// Public Definitions
+//--------------------------------------------------------------------------------------------------
+
+/// Memory Management interfaces.
+pub mod interface {
+
+    /// MMU functions.
+    pub trait MMU {
+        /// Called by the kernel during early init. Supposed to take the translation tables from the
+        /// `BSP`-supplied `virt_mem_layout()` and install/activate them for the respective MMU.
+        ///
+        /// # Safety
+        ///
+        /// - Changes the HW's global state.
+        unsafe fn init(&self) -> Result<(), &'static str>;
+    }
+}
+
+/// Architecture agnostic translation types.
+#[allow(missing_docs)]
+#[derive(Copy, Clone)]
+pub enum Translation {
+    Identity,
+    Offset(usize),
+}
+
+/// Architecture agnostic memory attributes.
+#[allow(missing_docs)]
+#[derive(Copy, Clone)]
+pub enum MemAttributes {
+    CacheableDRAM,
+    Device,
+}
+
+/// Architecture agnostic access permissions.
+#[allow(missing_docs)]
+#[derive(Copy, Clone)]
+pub enum AccessPermissions {
+    ReadOnly,
+    ReadWrite,
+}
+
+/// Collection of memory attributes.
+#[allow(missing_docs)]
+#[derive(Copy, Clone)]
+pub struct AttributeFields {
+    pub mem_attributes: MemAttributes,
+    pub acc_perms: AccessPermissions,
+    pub execute_never: bool,
+}
+
+/// Architecture agnostic descriptor for a memory range.
+#[allow(missing_docs)]
+pub struct TranslationDescriptor {
+    pub name: &'static str,
+    pub virtual_range: fn() -> RangeInclusive<usize>,
+    pub physical_range_translation: Translation,
+    pub attribute_fields: AttributeFields,
+}
+
+/// Type for expressing the kernel's virtual memory layout.
+pub struct KernelVirtualLayout<const NUM_SPECIAL_RANGES: usize> {
+    /// The last (inclusive) address of the address space.
+    max_virt_addr_inclusive: usize,
+
+    /// Array of descriptors for non-standard (normal cacheable DRAM) memory regions.
+    inner: [TranslationDescriptor; NUM_SPECIAL_RANGES],
+}
+
+//--------------------------------------------------------------------------------------------------
+// Public Code
+//--------------------------------------------------------------------------------------------------
+
+impl Default for AttributeFields {
+    fn default() -> AttributeFields {
+        AttributeFields {
+            mem_attributes: MemAttributes::CacheableDRAM,
+            acc_perms: AccessPermissions::ReadWrite,
+            execute_never: true,
+        }
+    }
+}
+
+/// Human-readable output of a TranslationDescriptor.
+impl fmt::Display for TranslationDescriptor {
+    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
+        // Call the function to which self.range points, and dereference the result, which causes
+        // Rust to copy the value.
+        let start = *(self.virtual_range)().start();
+        let end = *(self.virtual_range)().end();
+        let size = end - start + 1;
+
+        // log2(1024).
+        const KIB_RSHIFT: u32 = 10;
+
+        // log2(1024 * 1024).
+        const MIB_RSHIFT: u32 = 20;
+
+        let (size, unit) = if (size >> MIB_RSHIFT) > 0 {
+            (size >> MIB_RSHIFT, "MiB")
+        } else if (size >> KIB_RSHIFT) > 0 {
+            (size >> KIB_RSHIFT, "KiB")
+        } else {
+            (size, "Byte")
+        };
+
+        let attr = match self.attribute_fields.mem_attributes {
+            MemAttributes::CacheableDRAM => "C",
+            MemAttributes::Device => "Dev",
+        };
+
+        let acc_p = match self.attribute_fields.acc_perms {
+            AccessPermissions::ReadOnly => "RO",
+            AccessPermissions::ReadWrite => "RW",
+        };
+
+        let xn = if self.attribute_fields.execute_never {
+            "PXN"
+        } else {
+            "PX"
+        };
+
+        write!(
+            f,
+            "      {:#010x} - {:#010x} | {: >3} {} | {: <3} {} {: <3} | {}",
+            start, end, size, unit, attr, acc_p, xn, self.name
+        )
+    }
+}
+
+impl<const NUM_SPECIAL_RANGES: usize> KernelVirtualLayout<{ NUM_SPECIAL_RANGES }> {
+    /// Create a new instance.
+    pub const fn new(max: usize, layout: [TranslationDescriptor; NUM_SPECIAL_RANGES]) -> Self {
+        Self {
+            max_virt_addr_inclusive: max,
+            inner: layout,
+        }
+    }
+
+    /// For a virtual address, find and return the physical output address and corresponding
+    /// attributes.
+    ///
+    /// If the address is not found in `inner`, return an identity mapped default with normal
+    /// cacheable DRAM attributes.
+    pub fn virt_addr_properties(
+        &self,
+        virt_addr: usize,
+    ) -> Result<(usize, AttributeFields), &'static str> {
+        if virt_addr > self.max_virt_addr_inclusive {
+            return Err("Address out of range");
+        }
+
+        for i in self.inner.iter() {
+            if (i.virtual_range)().contains(&virt_addr) {
+                let output_addr = match i.physical_range_translation {
+                    Translation::Identity => virt_addr,
+                    Translation::Offset(a) => a + (virt_addr - (i.virtual_range)().start()),
+                };
+
+                return Ok((output_addr, i.attribute_fields));
+            }
+        }
+
+        Ok((virt_addr, AttributeFields::default()))
+    }
+
+    /// Print the memory layout.
+    pub fn print_layout(&self) {
+        use crate::info;
+
+        for i in self.inner.iter() {
+            info!("{}", i);
+        }
+    }
+}

diff -uNr 10_privilege_level/src/memory.rs 11_virtual_mem_part1_identity_mapping/src/memory.rs
--- 10_privilege_level/src/memory.rs
+++ 11_virtual_mem_part1_identity_mapping/src/memory.rs
@@ -4,6 +4,8 @@

 //! Memory Management.

+pub mod mmu;
+
 use core::ops::RangeInclusive;

 //--------------------------------------------------------------------------------------------------

```
