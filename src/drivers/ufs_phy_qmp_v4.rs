//! QMP v4 UFS PHY initialization tables from Linux kernel (phy-qcom-qmp-ufs.c)
//! Adapted for SM8150/QMP v4 compatible PHY (used by SM6375 with qcom,ufs-phy-qmp-v4)

use core::sync::atomic::{AtomicUsize, Ordering};

/// QMP v4 PHY register offsets (from phy-qcom-qmp-qserdes-com-v4.h, phy-qcom-qmp-qserdes-txrx-v4.h, phy-qcom-qmp-pcs-ufs-v4.h)

/// SERDES COM (common/PLL) registers
pub mod serdes_com {
    pub const ATB_SEL1: u32 = 0x000;
    pub const ATB_SEL2: u32 = 0x004;
    pub const FREQ_UPDATE: u32 = 0x008;
    pub const BG_TIMER: u32 = 0x00c;
    pub const SSC_EN_CENTER: u32 = 0x010;
    pub const SSC_ADJ_PER1: u32 = 0x014;
    pub const SSC_ADJ_PER2: u32 = 0x018;
    pub const SSC_PER1: u32 = 0x01c;
    pub const SSC_PER2: u32 = 0x020;
    pub const SSC_STEP_SIZE1_MODE0: u32 = 0x024;
    pub const SSC_STEP_SIZE2_MODE0: u32 = 0x028;
    pub const SSC_STEP_SIZE3_MODE0: u32 = 0x02c;
    pub const SSC_STEP_SIZE1_MODE1: u32 = 0x030;
    pub const SSC_STEP_SIZE2_MODE1: u32 = 0x034;
    pub const SSC_STEP_SIZE3_MODE1: u32 = 0x038;
    pub const POST_DIV: u32 = 0x03c;
    pub const POST_DIV_MUX: u32 = 0x040;
    pub const BIAS_EN_CLKBUFLR_EN: u32 = 0x044;
    pub const CLK_ENABLE1: u32 = 0x048;
    pub const SYS_CLK_CTRL: u32 = 0x04c;
    pub const SYSCLK_BUF_ENABLE: u32 = 0x050;
    pub const PLL_EN: u32 = 0x054;
    pub const PLL_IVCO: u32 = 0x058;
    pub const CMN_IETRIM: u32 = 0x05c;
    pub const CMN_IPTRIM: u32 = 0x060;
    pub const EP_CLOCK_DETECT_CTRL: u32 = 0x064;
    pub const SYSCLK_DET_COMP_STATUS: u32 = 0x068;
    pub const CLK_EP_DIV_MODE0: u32 = 0x06c;
    pub const CLK_EP_DIV_MODE1: u32 = 0x070;
    pub const CP_CTRL_MODE0: u32 = 0x074;
    pub const CP_CTRL_MODE1: u32 = 0x078;
    pub const PLL_RCTRL_MODE0: u32 = 0x07c;
    pub const PLL_RCTRL_MODE1: u32 = 0x080;
    pub const PLL_CCTRL_MODE0: u32 = 0x084;
    pub const PLL_CCTRL_MODE1: u32 = 0x088;
    pub const PLL_CNTRL: u32 = 0x08c;
    pub const BIAS_EN_CTRL_BY_PSM: u32 = 0x090;
    pub const SYSCLK_EN_SEL: u32 = 0x094;
    pub const CML_SYSCLK_SEL: u32 = 0x098;
    pub const RESETSM_CNTRL: u32 = 0x09c;
    pub const RESETSM_CNTRL2: u32 = 0x0a0;
    pub const LOCK_CMP_EN: u32 = 0x0a4;
    pub const LOCK_CMP_CFG: u32 = 0x0a8;
    pub const LOCK_CMP1_MODE0: u32 = 0x0ac;
    pub const LOCK_CMP2_MODE0: u32 = 0x0b0;
    pub const LOCK_CMP1_MODE1: u32 = 0x0b4;
    pub const LOCK_CMP2_MODE1: u32 = 0x0b8;
    pub const DEC_START_MODE0: u32 = 0x0bc;
    pub const DEC_START_MSB_MODE0: u32 = 0x0c0;
    pub const DEC_START_MODE1: u32 = 0x0c4;
    pub const DEC_START_MSB_MODE1: u32 = 0x0c8;
    pub const DIV_FRAC_START1_MODE0: u32 = 0x0cc;
    pub const DIV_FRAC_START2_MODE0: u32 = 0x0d0;
    pub const DIV_FRAC_START3_MODE0: u32 = 0x0d4;
    pub const DIV_FRAC_START1_MODE1: u32 = 0x0d8;
    pub const DIV_FRAC_START2_MODE1: u32 = 0x0dc;
    pub const DIV_FRAC_START3_MODE1: u32 = 0x0e0;
    pub const INTEGLOOP_INITVAL: u32 = 0x0e4;
    pub const INTEGLOOP_EN: u32 = 0x0e8;
    pub const INTEGLOOP_GAIN0_MODE0: u32 = 0x0ec;
    pub const INTEGLOOP_GAIN1_MODE0: u32 = 0x0f0;
    pub const INTEGLOOP_GAIN0_MODE1: u32 = 0x0f4;
    pub const INTEGLOOP_GAIN1_MODE1: u32 = 0x0f8;
    pub const INTEGLOOP_P_PATH_GAIN0: u32 = 0x0fc;
    pub const INTEGLOOP_P_PATH_GAIN1: u32 = 0x100;
    pub const VCOCAL_DEADMAN_CTRL: u32 = 0x104;
    pub const VCO_TUNE_CTRL: u32 = 0x108;
    pub const VCO_TUNE_MAP: u32 = 0x10c;
    pub const VCO_TUNE1_MODE0: u32 = 0x110;
    pub const VCO_TUNE2_MODE0: u32 = 0x114;
    pub const VCO_TUNE1_MODE1: u32 = 0x118;
    pub const VCO_TUNE2_MODE1: u32 = 0x11c;
    pub const VCO_TUNE_INITVAL1: u32 = 0x120;
    pub const VCO_TUNE_INITVAL2: u32 = 0x124;
    pub const VCO_TUNE_MINVAL1: u32 = 0x128;
    pub const VCO_TUNE_MINVAL2: u32 = 0x12c;
    pub const VCO_TUNE_MAXVAL1: u32 = 0x130;
    pub const VCO_TUNE_MAXVAL2: u32 = 0x134;
    pub const VCO_TUNE_TIMER1: u32 = 0x138;
    pub const VCO_TUNE_TIMER2: u32 = 0x13c;
    pub const CMN_STATUS: u32 = 0x140;
    pub const RESET_SM_STATUS: u32 = 0x144;
    pub const RESTRIM_CODE_STATUS: u32 = 0x148;
    pub const PLLCAL_CODE1_STATUS: u32 = 0x14c;
    pub const PLLCAL_CODE2_STATUS: u32 = 0x150;
    pub const CLK_SELECT: u32 = 0x154;
    pub const HSCLK_SEL: u32 = 0x158;
    pub const HSCLK_HS_SWITCH_SEL: u32 = 0x15c;
    pub const INTEGLOOP_BINCODE_STATUS: u32 = 0x160;
    pub const PLL_ANALOG: u32 = 0x164;
    pub const CORECLK_DIV_MODE0: u32 = 0x168;
    pub const CORECLK_DIV_MODE1: u32 = 0x16c;
    pub const SW_RESET: u32 = 0x170;
    pub const CORE_CLK_EN: u32 = 0x174;
    pub const C_READY_STATUS: u32 = 0x178;
    pub const CMN_CONFIG: u32 = 0x17c;
    pub const CMN_RATE_OVERRIDE: u32 = 0x180;
    pub const SVS_MODE_CLK_SEL: u32 = 0x184;
    pub const DEBUG_BUS0: u32 = 0x188;
    pub const DEBUG_BUS1: u32 = 0x18c;
    pub const DEBUG_BUS2: u32 = 0x190;
    pub const DEBUG_BUS3: u32 = 0x194;
    pub const DEBUG_BUS_SEL: u32 = 0x198;
    pub const CMN_MISC1: u32 = 0x19c;
    pub const CMN_MISC2: u32 = 0x1a0;
    pub const CMN_MODE: u32 = 0x1a4;
    pub const VCO_DC_LEVEL_CTRL: u32 = 0x1a8;
    pub const BIN_VCOCAL_CMP_CODE1_MODE0: u32 = 0x1ac;
    pub const BIN_VCOCAL_CMP_CODE2_MODE0: u32 = 0x1b0;
    pub const BIN_VCOCAL_CMP_CODE1_MODE1: u32 = 0x1b4;
    pub const BIN_VCOCAL_CMP_CODE2_MODE1: u32 = 0x1b8;
    pub const BIN_VCOCAL_HSCLK_SEL: u32 = 0x1bc;
}

/// TX registers
pub mod tx {
    pub const BIST_MODE_LANENO: u32 = 0x000;
    pub const BIST_INVERT: u32 = 0x004;
    pub const CLKBUF_ENABLE: u32 = 0x008;
    pub const TX_EMP_POST1_LVL: u32 = 0x00c;
    pub const TX_IDLE_LVL_LARGE_AMP: u32 = 0x010;
    pub const TX_DRV_LVL: u32 = 0x014;
    pub const TX_DRV_LVL_OFFSET: u32 = 0x018;
    pub const RESET_TSYNC_EN: u32 = 0x01c;
    pub const PRE_STALL_LDO_BOOST_EN: u32 = 0x020;
    pub const TX_BAND: u32 = 0x024;
    pub const SLEW_CNTL: u32 = 0x028;
    pub const INTERFACE_SELECT: u32 = 0x02c;
    pub const LPB_EN: u32 = 0x030;
    pub const RES_CODE_LANE_TX: u32 = 0x034;
    pub const RES_CODE_LANE_RX: u32 = 0x038;
    pub const RES_CODE_LANE_OFFSET_TX: u32 = 0x03c;
    pub const RES_CODE_LANE_OFFSET_RX: u32 = 0x040;
    pub const PERL_LENGTH1: u32 = 0x044;
    pub const PERL_LENGTH2: u32 = 0x048;
    pub const SERDES_BYP_EN_OUT: u32 = 0x04c;
    pub const DEBUG_BUS_SEL: u32 = 0x050;
    pub const TRANSCEIVER_BIAS_EN: u32 = 0x054;
    pub const HIGHZ_DRVR_EN: u32 = 0x058;
    pub const TX_POL_INV: u32 = 0x05c;
    pub const PARRATE_REC_DETECT_IDLE_EN: u32 = 0x060;
    pub const BIST_PATTERN1: u32 = 0x064;
    pub const BIST_PATTERN2: u32 = 0x068;
    pub const BIST_PATTERN3: u32 = 0x06c;
    pub const BIST_PATTERN4: u32 = 0x070;
    pub const BIST_PATTERN5: u32 = 0x074;
    pub const BIST_PATTERN6: u32 = 0x078;
    pub const BIST_PATTERN7: u32 = 0x07c;
    pub const BIST_PATTERN8: u32 = 0x080;
    pub const LANE_MODE_1: u32 = 0x084;
    pub const LANE_MODE_2: u32 = 0x088;
    pub const LANE_MODE_3: u32 = 0x08c;
    pub const ATB_SEL1: u32 = 0x090;
    pub const ATB_SEL2: u32 = 0x094;
    pub const RCV_DETECT_LVL: u32 = 0x098;
    pub const RCV_DETECT_LVL_2: u32 = 0x09c;
    pub const PRBS_SEED1: u32 = 0x0a0;
    pub const PRBS_SEED2: u32 = 0x0a4;
    pub const PRBS_SEED3: u32 = 0x0a8;
    pub const PRBS_SEED4: u32 = 0x0ac;
    pub const RESET_GEN: u32 = 0x0b0;
    pub const RESET_GEN_MUXES: u32 = 0x0b4;
    pub const TRAN_DRVR_EMP_EN: u32 = 0x0b8;
    pub const TX_INTERFACE_MODE: u32 = 0x0bc;
    pub const PWM_CTRL: u32 = 0x0c0;
    pub const PWM_ENCODED_OR_DATA: u32 = 0x0c4;
    pub const PWM_GEAR_1_DIVIDER_BAND2: u32 = 0x0c8;
    pub const PWM_GEAR_2_DIVIDER_BAND2: u32 = 0x0cc;
    pub const PWM_GEAR_3_DIVIDER_BAND2: u32 = 0x0d0;
    pub const PWM_GEAR_4_DIVIDER_BAND2: u32 = 0x0d4;
    pub const PWM_GEAR_1_DIVIDER_BAND0_1: u32 = 0x0d8;
    pub const PWM_GEAR_2_DIVIDER_BAND0_1: u32 = 0x0dc;
    pub const PWM_GEAR_3_DIVIDER_BAND0_1: u32 = 0x0e0;
    pub const PWM_GEAR_4_DIVIDER_BAND0_1: u32 = 0x0e4;
    pub const VMODE_CTRL1: u32 = 0x0e8;
    pub const ALOG_OBSV_BUS_CTRL_1: u32 = 0x0ec;
    pub const BIST_STATUS: u32 = 0x0f0;
    pub const BIST_ERROR_COUNT1: u32 = 0x0f4;
    pub const BIST_ERROR_COUNT2: u32 = 0x0f8;
    pub const ALOG_OBSV_BUS_STATUS_1: u32 = 0x0fc;
    pub const LANE_DIG_CONFIG: u32 = 0x100;
    pub const PI_QEC_CTRL: u32 = 0x104;
    pub const PRE_EMPH: u32 = 0x108;
    pub const SW_RESET: u32 = 0x10c;
    pub const DCC_OFFSET: u32 = 0x110;
    pub const DIG_BKUP_CTRL: u32 = 0x114;
    pub const DEBUG_BUS0: u32 = 0x118;
    pub const DEBUG_BUS1: u32 = 0x11c;
    pub const DEBUG_BUS2: u32 = 0x120;
    pub const DEBUG_BUS3: u32 = 0x124;
    pub const READ_EQCODE: u32 = 0x128;
    pub const READ_OFFSETCODE: u32 = 0x12c;
    pub const IA_ERROR_COUNTER_LOW: u32 = 0x130;
    pub const IA_ERROR_COUNTER_HIGH: u32 = 0x134;
    pub const VGA_READ_CODE: u32 = 0x138;
    pub const VTH_READ_CODE: u32 = 0x13c;
    pub const DFE_TAP1_READ_CODE: u32 = 0x140;
    pub const DFE_TAP2_READ_CODE: u32 = 0x144;
    pub const IDAC_STATUS_I: u32 = 0x148;
    pub const IDAC_STATUS_IBAR: u32 = 0x14c;
    pub const IDAC_STATUS_Q: u32 = 0x150;
    pub const IDAC_STATUS_QBAR: u32 = 0x154;
    pub const IDAC_STATUS_A: u32 = 0x158;
    pub const IDAC_STATUS_ABAR: u32 = 0x15c;
    pub const IDAC_STATUS_SM_ON: u32 = 0x160;
    pub const IDAC_STATUS_CAL_DONE: u32 = 0x164;
    pub const IDAC_STATUS_SIGNERROR: u32 = 0x168;
    pub const DCC_CAL_STATUS: u32 = 0x16c;
}

/// RX registers
pub mod rx {
    pub const UCDR_FO_GAIN_HALF: u32 = 0x000;
    pub const UCDR_FO_GAIN_QUARTER: u32 = 0x004;
    pub const UCDR_FO_GAIN: u32 = 0x008;
    pub const UCDR_SO_GAIN_HALF: u32 = 0x00c;
    pub const UCDR_SO_GAIN_QUARTER: u32 = 0x010;
    pub const UCDR_SO_GAIN: u32 = 0x014;
    pub const UCDR_SVS_FO_GAIN_HALF: u32 = 0x018;
    pub const UCDR_SVS_FO_GAIN_QUARTER: u32 = 0x01c;
    pub const UCDR_SVS_FO_GAIN: u32 = 0x020;
    pub const UCDR_SVS_SO_GAIN_HALF: u32 = 0x024;
    pub const UCDR_SVS_SO_GAIN_QUARTER: u32 = 0x028;
    pub const UCDR_SVS_SO_GAIN: u32 = 0x02c;
    pub const UCDR_FASTLOCK_FO_GAIN: u32 = 0x030;
    pub const UCDR_SO_SATURATION_AND_ENABLE: u32 = 0x034;
    pub const UCDR_FO_TO_SO_DELAY: u32 = 0x038;
    pub const UCDR_FASTLOCK_COUNT_LOW: u32 = 0x03c;
    pub const UCDR_FASTLOCK_COUNT_HIGH: u32 = 0x040;
    pub const UCDR_PI_CONTROLS: u32 = 0x044;
    pub const UCDR_PI_CTRL2: u32 = 0x048;
    pub const UCDR_SB2_THRESH1: u32 = 0x04c;
    pub const UCDR_SB2_THRESH2: u32 = 0x050;
    pub const UCDR_SB2_GAIN1: u32 = 0x054;
    pub const UCDR_SB2_GAIN2: u32 = 0x058;
    pub const AUX_CONTROL: u32 = 0x05c;
    pub const AUX_DATA_TCOARSE_TFINE: u32 = 0x060;
    pub const RCLK_AUXDATA_SEL: u32 = 0x064;
    pub const AC_JTAG_ENABLE: u32 = 0x068;
    pub const AC_JTAG_INITP: u32 = 0x06c;
    pub const AC_JTAG_INITN: u32 = 0x070;
    pub const AC_JTAG_LVL: u32 = 0x074;
    pub const AC_JTAG_MODE: u32 = 0x078;
    pub const AC_JTAG_RESET: u32 = 0x07c;
    pub const RX_TERM_BW: u32 = 0x080;
    pub const RX_RCVR_IQ_EN: u32 = 0x084;
    pub const RX_IDAC_I_DC_OFFSETS: u32 = 0x088;
    pub const RX_IDAC_IBAR_DC_OFFSETS: u32 = 0x08c;
    pub const RX_IDAC_Q_DC_OFFSETS: u32 = 0x090;
    pub const RX_IDAC_QBAR_DC_OFFSETS: u32 = 0x094;
    pub const RX_IDAC_A_DC_OFFSETS: u32 = 0x098;
    pub const RX_IDAC_ABAR_DC_OFFSETS: u32 = 0x09c;
    pub const RX_IDAC_EN: u32 = 0x0a0;
    pub const RX_IDAC_ENABLES: u32 = 0x0a4;
    pub const RX_IDAC_SIGN: u32 = 0x0a8;
    pub const RX_HIGHZ_HIGHRATE: u32 = 0x0ac;
    pub const RX_TERM_AC_BYPASS_DC_COUPLE_OFFSET: u32 = 0x0b0;
    pub const DFE_1: u32 = 0x0b4;
    pub const DFE_2: u32 = 0x0b8;
    pub const DFE_3: u32 = 0x0bc;
    pub const DFE_4: u32 = 0x0c0;
    pub const TX_ADAPT_PRE_THRESH1: u32 = 0x0c4;
    pub const TX_ADAPT_PRE_THRESH2: u32 = 0x0c8;
    pub const TX_ADAPT_POST_THRESH: u32 = 0x0cc;
    pub const TX_ADAPT_MAIN_THRESH: u32 = 0x0d0;
    pub const VGA_CAL_CNTRL1: u32 = 0x0d4;
    pub const VGA_CAL_CNTRL2: u32 = 0x0d8;
    pub const GM_CAL: u32 = 0x0dc;
    pub const RX_VGA_GAIN2_LSB: u32 = 0x0e0;
    pub const RX_VGA_GAIN2_MSB: u32 = 0x0e4;
    pub const RX_EQU_ADAPTOR_CNTRL1: u32 = 0x0e8;
    pub const RX_EQU_ADAPTOR_CNTRL2: u32 = 0x0ec;
    pub const RX_EQU_ADAPTOR_CNTRL3: u32 = 0x0f0;
    pub const RX_EQU_ADAPTOR_CNTRL4: u32 = 0x0f4;
    pub const RX_IDAC_TSETTLE_LOW: u32 = 0x0f8;
    pub const RX_IDAC_TSETTLE_HIGH: u32 = 0x0fc;
    pub const RX_IDAC_MEASURE_TIME: u32 = 0x100;
    pub const RX_IDAC_ACCUMULATOR: u32 = 0x104;
    pub const RX_EQ_OFFSET_LSB: u32 = 0x108;
    pub const RX_EQ_OFFSET_MSB: u32 = 0x10c;
    pub const RX_EQ_OFFSET_ADAPTOR_CNTRL1: u32 = 0x110;
    pub const RX_OFFSET_ADAPTOR_CNTRL2: u32 = 0x114;
    pub const SIGDET_ENABLES: u32 = 0x118;
    pub const SIGDET_CNTRL: u32 = 0x11c;
    pub const SIGDET_LVL: u32 = 0x120;
    pub const SIGDET_DEGLITCH_CNTRL: u32 = 0x124;
    pub const RX_BAND: u32 = 0x128;
    pub const CDR_FREEZE_UP_DN: u32 = 0x12c;
    pub const CDR_RESET_OVERRIDE: u32 = 0x130;
    pub const RX_INTERFACE_MODE: u32 = 0x134;
    pub const JITTER_GEN_MODE: u32 = 0x138;
    pub const SJ_AMP1: u32 = 0x13c;
    pub const SJ_AMP2: u32 = 0x140;
    pub const SJ_PER1: u32 = 0x144;
    pub const SJ_PER2: u32 = 0x148;
    pub const PPM_OFFSET1: u32 = 0x14c;
    pub const PPM_OFFSET2: u32 = 0x150;
    pub const SIGN_PPM_PERIOD1: u32 = 0x154;
    pub const SIGN_PPM_PERIOD2: u32 = 0x158;
    pub const RX_PWM_ENABLE_AND_DATA: u32 = 0x15c;
    pub const RX_PWM_GEAR1_TIMEOUT_COUNT: u32 = 0x160;
    pub const RX_PWM_GEAR2_TIMEOUT_COUNT: u32 = 0x164;
    pub const RX_PWM_GEAR3_TIMEOUT_COUNT: u32 = 0x168;
    pub const RX_PWM_GEAR4_TIMEOUT_COUNT: u32 = 0x16c;
    pub const RX_MODE_00_LOW: u32 = 0x170;
    pub const RX_MODE_00_HIGH: u32 = 0x174;
    pub const RX_MODE_00_HIGH2: u32 = 0x178;
    pub const RX_MODE_00_HIGH3: u32 = 0x17c;
    pub const RX_MODE_00_HIGH4: u32 = 0x180;
    pub const RX_MODE_01_LOW: u32 = 0x184;
    pub const RX_MODE_01_HIGH: u32 = 0x188;
    pub const RX_MODE_01_HIGH2: u32 = 0x18c;
    pub const RX_MODE_01_HIGH3: u32 = 0x190;
    pub const RX_MODE_01_HIGH4: u32 = 0x194;
    pub const RX_MODE_10_LOW: u32 = 0x198;
    pub const RX_MODE_10_HIGH: u32 = 0x19c;
    pub const RX_MODE_10_HIGH2: u32 = 0x1a0;
    pub const RX_MODE_10_HIGH3: u32 = 0x1a4;
    pub const RX_MODE_10_HIGH4: u32 = 0x1a8;
    pub const PHPRE_CTRL: u32 = 0x1ac;
    pub const PHPRE_INITVAL: u32 = 0x1b0;
    pub const DFE_EN_TIMER: u32 = 0x1b4;
    pub const DFE_CTLE_POST_CAL_OFFSET: u32 = 0x1b8;
    pub const DCC_CTRL1: u32 = 0x1bc;
    pub const DCC_CTRL2: u32 = 0x1c0;
    pub const VTH_CODE: u32 = 0x1c4;
    pub const VTH_MIN_THRESH: u32 = 0x1c8;
    pub const VTH_MAX_THRESH: u32 = 0x1cc;
    pub const ALOG_OBSV_BUS_CTRL_1: u32 = 0x1d0;
    pub const PI_CTRL1: u32 = 0x1d4;
    pub const PI_CTRL2: u32 = 0x1d8;
    pub const PI_QUAD: u32 = 0x1dc;
    pub const IDATA1: u32 = 0x1e0;
    pub const IDATA2: u32 = 0x1e4;
    pub const AUX_DATA1: u32 = 0x1e8;
    pub const AUX_DATA2: u32 = 0x1ec;
    pub const AC_JTAG_OUTP: u32 = 0x1f0;
    pub const AC_JTAG_OUTN: u32 = 0x1f4;
    pub const RX_SIGDET: u32 = 0x1f8;
    pub const ALOG_OBSV_BUS_STATUS_1: u32 = 0x1fc;
}

/// PCS UFS registers
pub mod pcs {
    pub const PHY_START: u32 = 0x000;
    pub const POWER_DOWN_CONTROL: u32 = 0x004;
    pub const SW_RESET: u32 = 0x008;
    pub const TIMER_20US_CORECLK_STEPS_MSB: u32 = 0x00c;
    pub const TIMER_20US_CORECLK_STEPS_LSB: u32 = 0x010;
    pub const PLL_CNTL: u32 = 0x02c;
    pub const TX_LARGE_AMP_DRV_LVL: u32 = 0x030;
    pub const TX_SMALL_AMP_DRV_LVL: u32 = 0x038;
    pub const BIST_FIXED_PAT_CTRL: u32 = 0x060;
    pub const TX_HSGEAR_CAPABILITY: u32 = 0x074;
    pub const RX_HSGEAR_CAPABILITY: u32 = 0x0b4;
    pub const DEBUG_BUS_CLKSEL: u32 = 0x124;
    pub const LINECFG_DISABLE: u32 = 0x148;
    pub const RX_MIN_HIBERN8_TIME: u32 = 0x150;
    pub const RX_SIGDET_CTRL2: u32 = 0x158;
    pub const TX_PWM_GEAR_BAND: u32 = 0x160;
    pub const TX_HS_GEAR_BAND: u32 = 0x168;
    pub const READY_STATUS: u32 = 0x180;
    pub const TX_MID_TERM_CTRL1: u32 = 0x1d8;
    pub const MULTI_LANE_CTRL1: u32 = 0x1e0;
}

/// PHY initialization entry
#[derive(Clone, Copy)]
pub struct PhyInitEntry {
    pub offset: u32,
    pub value: u32,
}

/// Lane mask for multi-lane PHYs
pub const LANE_MASK_0: u8 = 0x01;
pub const LANE_MASK_1: u8 = 0x02;
pub const LANE_MASK_BOTH: u8 = 0x03;

/// SM8150 / QMP v4 UFS PHY SERDES initialization table (Rate A + Rate B)
/// From Linux: sm8150_ufsphy_serdes_tbl
pub const SERDES_INIT_TABLE: &[PhyInitEntry] = &[
    // Rate A (HS-G3 / Gear 4)
    PhyInitEntry { offset: serdes_com::SYSCLK_EN_SEL, value: 0xd9 },
    PhyInitEntry { offset: serdes_com::HSCLK_SEL, value: 0x11 },
    PhyInitEntry { offset: serdes_com::HSCLK_HS_SWITCH_SEL, value: 0x00 },
    PhyInitEntry { offset: serdes_com::LOCK_CMP_EN, value: 0x01 },
    PhyInitEntry { offset: serdes_com::VCO_TUNE_MAP, value: 0x02 },
    PhyInitEntry { offset: serdes_com::PLL_IVCO, value: 0x0f },
    PhyInitEntry { offset: serdes_com::VCO_TUNE_INITVAL2, value: 0x00 },
    PhyInitEntry { offset: serdes_com::BIN_VCOCAL_HSCLK_SEL, value: 0x11 },
    PhyInitEntry { offset: serdes_com::DEC_START_MODE0, value: 0x82 },
    PhyInitEntry { offset: serdes_com::CP_CTRL_MODE0, value: 0x06 },
    PhyInitEntry { offset: serdes_com::PLL_RCTRL_MODE0, value: 0x16 },
    PhyInitEntry { offset: serdes_com::PLL_CCTRL_MODE0, value: 0x36 },
    PhyInitEntry { offset: serdes_com::LOCK_CMP1_MODE0, value: 0xff },
    PhyInitEntry { offset: serdes_com::LOCK_CMP2_MODE0, value: 0x0c },
    PhyInitEntry { offset: serdes_com::BIN_VCOCAL_CMP_CODE1_MODE0, value: 0xac },
    PhyInitEntry { offset: serdes_com::BIN_VCOCAL_CMP_CODE2_MODE0, value: 0x1e },
    PhyInitEntry { offset: serdes_com::DEC_START_MODE1, value: 0x98 },
    PhyInitEntry { offset: serdes_com::CP_CTRL_MODE1, value: 0x06 },
    PhyInitEntry { offset: serdes_com::PLL_RCTRL_MODE1, value: 0x16 },
    PhyInitEntry { offset: serdes_com::PLL_CCTRL_MODE1, value: 0x36 },
    PhyInitEntry { offset: serdes_com::LOCK_CMP1_MODE1, value: 0x32 },
    PhyInitEntry { offset: serdes_com::LOCK_CMP2_MODE1, value: 0x0f },
    PhyInitEntry { offset: serdes_com::BIN_VCOCAL_CMP_CODE1_MODE1, value: 0xdd },
    PhyInitEntry { offset: serdes_com::BIN_VCOCAL_CMP_CODE2_MODE1, value: 0x23 },
    // Rate B (HS-G4 / Gear 5) - different VCO_TUNE_MAP
    PhyInitEntry { offset: serdes_com::VCO_TUNE_MAP, value: 0x06 },
];

/// SM8150 / QMP v4 UFS PHY TX initialization table
/// From Linux: sm8150_ufsphy_tx_tbl
pub const TX_INIT_TABLE: &[PhyInitEntry] = &[
    PhyInitEntry { offset: tx::PWM_GEAR_1_DIVIDER_BAND0_1, value: 0x06 },
    PhyInitEntry { offset: tx::PWM_GEAR_2_DIVIDER_BAND0_1, value: 0x03 },
    PhyInitEntry { offset: tx::PWM_GEAR_3_DIVIDER_BAND0_1, value: 0x01 },
    PhyInitEntry { offset: tx::PWM_GEAR_4_DIVIDER_BAND0_1, value: 0x00 },
    PhyInitEntry { offset: tx::LANE_MODE_1, value: 0x05 },
    PhyInitEntry { offset: tx::TRAN_DRVR_EMP_EN, value: 0x0c },
];

/// SM8150 / QMP v4 UFS PHY RX initialization table
/// From Linux: sm8150_ufsphy_rx_tbl
pub const RX_INIT_TABLE: &[PhyInitEntry] = &[
    PhyInitEntry { offset: rx::SIGDET_LVL, value: 0x24 },
    PhyInitEntry { offset: rx::SIGDET_CNTRL, value: 0x0f },
    PhyInitEntry { offset: rx::SIGDET_DEGLITCH_CNTRL, value: 0x1e },
    PhyInitEntry { offset: rx::RX_BAND, value: 0x18 },
    PhyInitEntry { offset: rx::UCDR_FASTLOCK_FO_GAIN, value: 0x0a },
    PhyInitEntry { offset: rx::UCDR_SO_SATURATION_AND_ENABLE, value: 0x4b },
    PhyInitEntry { offset: rx::UCDR_PI_CONTROLS, value: 0xf1 },
    PhyInitEntry { offset: rx::UCDR_FASTLOCK_COUNT_LOW, value: 0x80 },
    PhyInitEntry { offset: rx::UCDR_PI_CTRL2, value: 0x80 },
    PhyInitEntry { offset: rx::UCDR_FO_GAIN, value: 0x0c },
    PhyInitEntry { offset: rx::UCDR_SO_GAIN, value: 0x04 },
    PhyInitEntry { offset: rx::RX_TERM_BW, value: 0x1b },
    PhyInitEntry { offset: rx::RX_EQU_ADAPTOR_CNTRL2, value: 0x06 },
    PhyInitEntry { offset: rx::RX_EQU_ADAPTOR_CNTRL3, value: 0x04 },
    PhyInitEntry { offset: rx::RX_EQU_ADAPTOR_CNTRL4, value: 0x1d },
    PhyInitEntry { offset: rx::RX_OFFSET_ADAPTOR_CNTRL2, value: 0x00 },
    PhyInitEntry { offset: rx::RX_IDAC_MEASURE_TIME, value: 0x10 },
    PhyInitEntry { offset: rx::RX_IDAC_TSETTLE_LOW, value: 0xc0 },
    PhyInitEntry { offset: rx::RX_IDAC_TSETTLE_HIGH, value: 0x00 },
    PhyInitEntry { offset: rx::RX_MODE_00_LOW, value: 0x36 },
    PhyInitEntry { offset: rx::RX_MODE_00_HIGH, value: 0x36 },
    PhyInitEntry { offset: rx::RX_MODE_00_HIGH2, value: 0xf6 },
    PhyInitEntry { offset: rx::RX_MODE_00_HIGH3, value: 0x3b },
    PhyInitEntry { offset: rx::RX_MODE_00_HIGH4, value: 0x3d },
    PhyInitEntry { offset: rx::RX_MODE_01_LOW, value: 0xe0 },
    PhyInitEntry { offset: rx::RX_MODE_01_HIGH, value: 0xc8 },
    PhyInitEntry { offset: rx::RX_MODE_01_HIGH2, value: 0xc8 },
    PhyInitEntry { offset: rx::RX_MODE_01_HIGH3, value: 0x3b },
    PhyInitEntry { offset: rx::RX_MODE_01_HIGH4, value: 0xb1 },
    PhyInitEntry { offset: rx::RX_MODE_10_LOW, value: 0xe0 },
    PhyInitEntry { offset: rx::RX_MODE_10_HIGH, value: 0xc8 },
    PhyInitEntry { offset: rx::RX_MODE_10_HIGH2, value: 0xc8 },
    PhyInitEntry { offset: rx::RX_MODE_10_HIGH3, value: 0x3b },
    PhyInitEntry { offset: rx::RX_MODE_10_HIGH4, value: 0xb1 },
];

/// SM8150 / QMP v4 UFS PHY PCS initialization table
/// From Linux: sm8150_ufsphy_pcs_tbl
pub const PCS_INIT_TABLE: &[PhyInitEntry] = &[
    PhyInitEntry { offset: pcs::RX_SIGDET_CTRL2, value: 0x6d },
    PhyInitEntry { offset: pcs::TX_LARGE_AMP_DRV_LVL, value: 0x0a },
    PhyInitEntry { offset: pcs::TX_SMALL_AMP_DRV_LVL, value: 0x02 },
    PhyInitEntry { offset: pcs::TX_MID_TERM_CTRL1, value: 0x43 },
    PhyInitEntry { offset: pcs::DEBUG_BUS_CLKSEL, value: 0x1f },
    PhyInitEntry { offset: pcs::RX_MIN_HIBERN8_TIME, value: 0xff },
    PhyInitEntry { offset: pcs::MULTI_LANE_CTRL1, value: 0x02 },
];

/// PHY base addresses (relative to PHY_BASE)
/// Typical layout for QMP v4 UFS PHY:
/// - SERDES COM: PHY_BASE + 0x0000
/// - TX lane 0:  PHY_BASE + 0x1000
/// - RX lane 0:  PHY_BASE + 0x2000
/// - PCS lane 0: PHY_BASE + 0x3000
pub const SERDES_OFFSET: usize = 0x0000;
pub const TX_LANE0_OFFSET: usize = 0x1000;
pub const RX_LANE0_OFFSET: usize = 0x2000;
pub const PCS_LANE0_OFFSET: usize = 0x3000;

/// Global PHY base address (set during probe)
static PHY_BASE: AtomicUsize = AtomicUsize::new(0);

/// Set PHY base address
pub fn set_phy_base(base: usize) {
    PHY_BASE.store(base, Ordering::Release);
}

/// Write to SERDES COM register
pub fn serdes_wr(offset: u32, value: u32) {
    let base = PHY_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    unsafe {
        core::ptr::write_volatile((base + SERDES_OFFSET + offset as usize) as *mut u32, value);
    }
}

/// Write to TX lane 0 register
pub fn tx_wr(offset: u32, value: u32) {
    let base = PHY_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    unsafe {
        core::ptr::write_volatile((base + TX_LANE0_OFFSET + offset as usize) as *mut u32, value);
    }
}

/// Write to RX lane 0 register
pub fn rx_wr(offset: u32, value: u32) {
    let base = PHY_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    unsafe {
        core::ptr::write_volatile((base + RX_LANE0_OFFSET + offset as usize) as *mut u32, value);
    }
}

/// Write to PCS lane 0 register
pub fn pcs_wr(offset: u32, value: u32) {
    let base = PHY_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    unsafe {
        core::ptr::write_volatile((base + PCS_LANE0_OFFSET + offset as usize) as *mut u32, value);
    }
}

/// Read from PCS lane 0 register
pub fn pcs_rd(offset: u32) -> u32 {
    let base = PHY_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return 0;
    }
    unsafe {
        core::ptr::read_volatile((base + PCS_LANE0_OFFSET + offset as usize) as *const u32)
    }
}

/// Initialize QMP v4 UFS PHY (complete sequence from Linux)
/// Returns true on success
pub fn phy_init() -> bool {
    let base = PHY_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return false;
    }

    // Map all PHY regions (4KB each)
    crate::mmu::map_device_identity(base & !0xfff, (base & !0xfff) + 0x4000);

    // 1. SERDES COM initialization (PLL configuration)
    for entry in SERDES_INIT_TABLE {
        serdes_wr(entry.offset, entry.value);
    }

    // 2. TX lane initialization
    for entry in TX_INIT_TABLE {
        tx_wr(entry.offset, entry.value);
    }

    // 3. RX lane initialization
    for entry in RX_INIT_TABLE {
        rx_wr(entry.offset, entry.value);
    }

    // 4. PCS initialization
    for entry in PCS_INIT_TABLE {
        pcs_wr(entry.offset, entry.value);
    }

    // 5. Bring PHY out of reset and start
    // Clear SW_RESET in PCS
    pcs_wr(pcs::SW_RESET, 0);

    // Enable SERDES_START | PCS_START
    pcs_wr(pcs::PHY_START, 0x3);

    // Wait for PCS_READY
    use crate::timer;
    let deadline = timer::counter() + timer::freq() / 100; // 10ms timeout
    loop {
        let status = pcs_rd(pcs::READY_STATUS);
        if status & 0x1 != 0 {
            break;
        }
        if timer::counter() > deadline {
            return false;
        }
        core::hint::spin_loop();
    }

    crate::kprintln!("ufs: QMP v4 PHY init done (SM8150 tables)");
    true
}