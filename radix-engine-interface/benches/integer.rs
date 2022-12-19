use std::{concat, str::FromStr};

use criterion::{BenchmarkId, Criterion};
use radix_engine_interface::math::{NthRoot,I256,I512};
use num_traits::Pow;
use num_bigint::BigInt;
use rug::{Integer, ops::Pow as RugPow};

use crate::{bench_ops,process_op};
use crate::macros::QUICK;

macro_rules! ops_fn {
    ($t:ty, $root_fn:ident) => {
        paste::item! {
            fn [< $t:lower _add >](a: &$t, b: &$t) {
                let _ = a + b;
            }

            fn [< $t:lower _sub >](a: &$t, b: &$t) {
                let _ = a - b;
            }

            fn [< $t:lower _mul >](a: &$t, b: &$t) {
                let _ = a * b;
            }

            fn [< $t:lower _div >](a: &$t, b: &$t) {
                let _ = a / b;
            }

            fn [< $t:lower _root >](a: &$t, n: &u32) {
                let _ = a.$root_fn(*n);
            }

            fn [< $t:lower _pow >](a: &$t, exp: &u32) {
                let _ = a.pow(*exp);
            }

            fn [< $t:lower _to_string >](a: &$t, _: &str) {
                let _ = a.to_string();
            }

            fn [< $t:lower _from_string >](s: &str, _: &str) {
                let _ = <$t>::from_str(s).unwrap();
            }
        }
    };
    ($t:ty, $root_fn:ident, "clone") => {
        paste::item! {
            fn [< $t:lower _add >](a: &$t, b: &$t) {
                let _ = a.clone() + b.clone();
            }

            fn [< $t:lower _sub >](a: &$t, b: &$t) {
                let _ = a.clone() - b.clone();
            }

            fn [< $t:lower _mul >](a: &$t, b: &$t) {
                let _ = a.clone() * b.clone();
            }

            fn [< $t:lower _div >](a: &$t, b: &$t) {
                let _ = a.clone() / b.clone();
            }

            fn [< $t:lower _root >](a: &$t, n: &u32) {
                let _ = a.clone().$root_fn(*n);
            }

            fn [< $t:lower _pow >](a: &$t, exp: &u32) {
                let _ = a.pow(*exp);
            }

            fn [< $t:lower _to_string >](a: &$t, _: &str) {
                let _ = a.to_string();
            }

            fn [< $t:lower _from_string >](s: &str, _: &str) {
                let _ = <$t>::from_str(s).unwrap();
            }
        }
    };
}

const ADD_OPERANDS: [(&str, &str); 4] = [
    ("278960446186580977117854925043439539266349000000000000000000000000000000000", "278960446186580977117854925043439539266349000000000000000000000000000000000"),
    ("-278960446186580977117854925043439539266349000000000000000000000000000000000", "278960446186580977117854925043439539266349000000000000000000000000000000000"),
    ("1", "-1"),
    ("-278960446186580977117854925043439539266349000000000000000000000000000000000", "-278960446186580977117854925043439539266349000000000000000000000000000000000"),
];

const SUB_OPERANDS: [(&str, &str); 4] = [
    ("278960446186580977117854925043439539266349000000000000000000000000000000000", "278960446186580977117854925043439539266349000000000000000000000000000000000"),
    ("-278960446186580977117854925043439539266349000000000000000000000000000000000", "278960446186580977117854925043439539266349000000000000000000000000000000000"),
    ("1", "-1"),
    ("-278960446186580977117854925043439539266349000000000000000000000000000000000", "-278960446186580977117854925043439539266349000000000000000000000000000000000"),
];

const MUL_OPERANDS: [(&str, &str); 4] = [
    ("278960446186580977117854925043439539", "2789604461865809771178549250434395392"),
    ("-278960446186580977117854925043439539", "2789604461865809771178549250434395392"),
    ("634992332820282019728", "131231233"),
    ("-123123123123", "-1"),
];

const DIV_OPERANDS: [(&str, &str); 4] = [
    ("278960446186580977117854925043439539", "2789604461865809771178549250434395392"),
    ("-278960446186580977117854925043439539", "2789604461865809771178549250434395392"),
    ("634992332820282019728", "131231233"),
    ("-123123123123", "-1"),
];

const ROOT_OPERANDS: [(&str, &str); 4] = [
    ("57896044618658097711785492504343953926634992332820282019728","17"),
    ("12379879872423987", "13"),
    ("12379879872423987", "5"),
    ("9", "2"),
];

const POW_OPERANDS: [(&str, &str); 4] = [
    ("12", "13"),
    ("1123123123", "5"),
    ("4", "5"),
    ("9", "2"),
];

const TO_STRING_OPERANDS: [&str; 4] = [
    "578960446186580977117854925043439539266349923328202820197792003956564819967",
    "-112379878901230908903281928379813",
    "12379879872423987123123123",
    "9",
];

const FROM_STRING_OPERANDS: [&str; 4] = [
    "578960446186580977117854925043439539266349923328202820197792003956564819967",
    "-112379878901230908903281928379813",
    "12379879872423987123123123",
    "9",
];

ops_fn!(I256, nth_root);
bench_ops!(I256, "add");
bench_ops!(I256, "sub");
bench_ops!(I256, "mul");
bench_ops!(I256, "div");
bench_ops!(I256, "root", u32);
bench_ops!(I256, "pow", u32);
bench_ops!(I256, "to_string");
bench_ops!(I256, "from_string");

ops_fn!(I512, nth_root);
bench_ops!(I512, "add");
bench_ops!(I512, "sub");
bench_ops!(I512, "mul");
bench_ops!(I512, "div");
bench_ops!(I512, "root", u32);
bench_ops!(I512, "pow", u32);
bench_ops!(I512, "to_string");
bench_ops!(I512, "from_string");

ops_fn!(BigInt, nth_root);
bench_ops!(BigInt, "add");
bench_ops!(BigInt, "sub");
bench_ops!(BigInt, "mul");
bench_ops!(BigInt, "div");
bench_ops!(BigInt, "root", u32);
bench_ops!(BigInt, "pow", u32);
bench_ops!(BigInt, "to_string");
bench_ops!(BigInt, "from_string");

ops_fn!(Integer, root, "clone");
bench_ops!(Integer, "add");
bench_ops!(Integer, "sub");
bench_ops!(Integer, "mul");
bench_ops!(Integer, "div");
bench_ops!(Integer, "root", u32);
bench_ops!(Integer, "pow", u32);
bench_ops!(Integer, "to_string");
bench_ops!(Integer, "from_string");
