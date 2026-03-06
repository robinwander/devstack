import type { Variants } from "framer-motion";

export const springs = {
  snappy: { type: "spring" as const, stiffness: 400, damping: 25, mass: 0.5 },
  default: { type: "spring" as const, stiffness: 300, damping: 30, mass: 1 },
  gentle: { type: "spring" as const, stiffness: 120, damping: 14, mass: 1 },
  bouncy: { type: "spring" as const, stiffness: 600, damping: 15, mass: 1 },
};

export const fadeInUp: Variants = {
  hidden: { opacity: 0, y: 12 },
  visible: {
    opacity: 1,
    y: 0,
    transition: { duration: 0.3, ease: "easeOut" },
  },
};

export const staggerContainer: Variants = {
  hidden: { opacity: 0 },
  visible: {
    opacity: 1,
    transition: {
      staggerChildren: 0.06,
      delayChildren: 0.1,
    },
  },
};

export const staggerItem: Variants = {
  hidden: { opacity: 0, y: 8 },
  visible: {
    opacity: 1,
    y: 0,
    transition: { duration: 0.25, ease: "easeOut" },
  },
};

export const scaleIn: Variants = {
  hidden: { opacity: 0, scale: 0.9 },
  visible: {
    opacity: 1,
    scale: 1,
    transition: springs.snappy,
  },
};

export const pulse: Variants = {
  pulse: {
    scale: [1, 1.15, 1],
    opacity: [1, 0.8, 1],
    transition: {
      duration: 2,
      repeat: Infinity,
      ease: "easeInOut",
    },
  },
};

export const spin: Variants = {
  spin: {
    rotate: 360,
    transition: {
      duration: 1.2,
      repeat: Infinity,
      ease: "linear",
    },
  },
};

export const blink: Variants = {
  blink: {
    opacity: [1, 0.3, 1],
    transition: {
      duration: 1,
      repeat: Infinity,
      ease: "easeInOut",
    },
  },
};
