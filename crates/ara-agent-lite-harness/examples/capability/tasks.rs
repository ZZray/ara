//! Fresh small fixtures and independent validators. These are not the old S3 task.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, path::Path};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Coding,
    Reasoning,
    Reverse,
}

pub const CODING_INITIAL:&str="#include \"window.hpp\"\nint phase(int tick,int cycle){return tick%cycle;}\nbool in_window(int tick,int start,int width,int cycle){int p=phase(tick,cycle);return p>=start&&p<=start+width;}\n";
pub const CODING_CORRECT:&str="#include \"window.hpp\"\nint phase(int tick,int cycle){int r=tick%cycle;return r<0?r+cycle:r;}\nbool in_window(int tick,int start,int width,int cycle){return width>0&&phase(phase(tick,cycle)-phase(start,cycle),cycle)<width;}\n";
pub const REVERSE_INITIAL: &str = "#include <cstdint>\nstd::uint32_t recover(std::uint32_t x){return x;}\n";
pub const REVERSE_CORRECT: &str =
    "#include <cstdint>\nstd::uint32_t recover(std::uint32_t x){return (((x<<5)|(x>>27))^0x6d2b79f5u)+0x013579bdu;}\n";
pub const REVERSE_SOURCE:&str="#include <cstdint>\nextern \"C\" std::uint32_t transform(std::uint32_t x){return (((x<<5)|(x>>27))^0x6d2b79f5u)+0x013579bdu;}\n";
pub fn candidate(kind: Kind) -> &'static str {
    match kind {
        Kind::Coding => "window.cpp",
        Kind::Reasoning => "plan.json",
        Kind::Reverse => "recovery.cpp",
    }
}
pub fn correct(kind: Kind) -> String {
    match kind {
        Kind::Coding => CODING_CORRECT.into(),
        Kind::Reverse => REVERSE_CORRECT.into(),
        Kind::Reasoning => {
            json!({"starts":{"A":0,"B":0,"C":2,"D":4},"makespan":5,"critical_path":["A","C","D"]}).to_string()
        }
    }
}
pub fn wrong(kind: Kind) -> String {
    match kind {
        Kind::Coding => CODING_INITIAL.into(),
        Kind::Reverse => REVERSE_INITIAL.into(),
        Kind::Reasoning => {
            json!({"starts":{"A":0,"B":0,"C":2,"D":3},"makespan":4,"critical_path":["A","C","D"]}).to_string()
        }
    }
}
pub fn objective(kind: Kind) -> &'static str {
    match kind{
 Kind::Coding=>"Repair the circular-window C++ implementation in window.cpp. Read task.md, window.hpp and demo.cpp; only window.cpp is writable. Use write_task to submit actual source, check_candidate for real compilation/behavior, and finish after submitting. Host independently tests hidden inputs. Do not try to read Host/oracle/configuration files. Preserve the declared API and preconditions.",
 Kind::Reasoning=>"Read task.md and produce the constrained minimum-makespan schedule in plan.json via write_task. All constraints are in the task. check_candidate validates the actual JSON. A prose claim is not submission. If Host feedback requests changes, correct the same artifact in the same Session.",
 Kind::Reverse=>"Recover the unsigned 32-bit integer transform in the supplied stripped mystery.o. Read task.md and use inspect_binary for actual GNU objdump disassembly. Write C++ recovery.cpp implementing std::uint32_t recover(std::uint32_t). No access is granted to original source or oracle. check_candidate really compiles and checks independent inputs. Submit actual source and finish; no model-authored shell is supported."
}
}
pub fn prepare(kind: Kind, workspace: &Path, private: &Path) -> Result<()> {
    match kind {
        Kind::Coding => {
            fs::write(workspace.join("task.md"),"Implement int phase(int tick,int cycle) returning the unique residue in [0,cycle). Implement bool in_window(int tick,int start,int width,int cycle): true iff tick lies in the circular half-open window starting at start and spanning width residues. Normalize tick and start; width 0 is empty, width==cycle is full. Preconditions: cycle in 1..10000, width in 0..cycle, tick/start in -100000..100000. Preserve functions and header. Fix window.cpp; demo.cpp is a separate consumer and must remain unchanged.\n")?;
            fs::write(workspace.join("window.hpp"),"#pragma once\nint phase(int tick,int cycle);\nbool in_window(int tick,int start,int width,int cycle);\n")?;
            fs::write(workspace.join("window.cpp"), CODING_INITIAL)?;
            fs::write(workspace.join("demo.cpp"),"#include \"window.hpp\"\n#include <iostream>\nint main(){std::cout<<phase(-1,8)<<' '<<in_window(0,7,2,8)<<'\\n';}\n")?;
        }
        Kind::Reasoning => {
            fs::write(workspace.join("task.md"),"Schedule four non-preemptive jobs A,B,C,D on two identical workers. Durations are A=2,B=3,C=2,D=1. All starts must be integer 0..8. C starts after A ends. D starts after both B and C end. At most two jobs may execute simultaneously; intervals are half-open. Minimize makespan. Write exactly {\"starts\":{\"A\":integer,\"B\":integer,\"C\":integer,\"D\":integer},\"makespan\":integer,\"critical_path\":[job IDs]}. critical_path must be a true dependency path whose durations sum to the optimal makespan. No other fields.\n")?;
            fs::write(workspace.join("plan.json"), "{}\n")?;
        }
        Kind::Reverse => {
            fs::write(workspace.join("task.md"),"mystery.o is a self-owned stripped GNU C++ object containing one integer transformation. Arithmetic is modulo 2^32. GNU objdump output is available through inspect_binary; recover the transformation and write recovery.cpp including <cstdint> and defining std::uint32_t recover(std::uint32_t x). Do not add a main function. Original source and independent input oracle are not granted.\n")?;
            fs::write(workspace.join("recovery.cpp"), REVERSE_INITIAL)?;
            fs::write(private.join("original-transform.cpp"), REVERSE_SOURCE)?;
        }
    }
    Ok(())
}
pub fn read_paths(kind: Kind) -> Vec<&'static str> {
    match kind {
        Kind::Coding => vec!["task.md", "window.hpp", "window.cpp", "demo.cpp"],
        Kind::Reasoning => vec!["task.md", "plan.json"],
        Kind::Reverse => vec!["task.md", "recovery.cpp"],
    }
}
pub fn build_oracle(kind: Kind) -> (String, Vec<String>) {
    match kind {
        Kind::Coding => {
            let mut cases = vec![
                (-1, 7, 2, 8),
                (0, 7, 2, 8),
                (1, 7, 2, 8),
                (7, 7, 0, 8),
                (7, 7, 8, 8),
                (-99999, -17, 3, 13),
                (99999, 99998, 1, 2),
            ];
            for index in 0..53 {
                let cycle = 1 + (index * 17) % 97;
                cases.push((index * 173 - 4000, index * 29 - 700, (index * 13) % (cycle + 1), cycle));
            }
            let mut code = String::from("#include \"window.hpp\"\n#include <iostream>\nint main(){\n");
            let mut expected = Vec::new();
            for (tick, start, width, cycle) in cases {
                let p = ((tick % cycle) + cycle) % cycle;
                let s = ((start % cycle) + cycle) % cycle;
                let inside = width > 0 && ((p - s + cycle) % cycle) < width;
                code.push_str(&format!(
                    "std::cout<<phase({tick},{cycle})<<','<<(in_window({tick},{start},{width},{cycle})?1:0)<<'\\n';\n"
                ));
                expected.push(format!("{p},{}", i32::from(inside)));
            }
            code.push_str("}\n");
            (code, expected)
        }
        Kind::Reverse => {
            let mut inputs = vec![0u32, 1, u32::MAX, 0x80000000, 31, 32, 127, 128, 257, 65535, 1000000007];
            let mut value = 0x815723abu32;
            for _ in 0..53 {
                value = value.wrapping_mul(1664525).wrapping_add(1013904223);
                inputs.push(value);
            }
            let mut code = String::from(
                "#include <cstdint>\n#include <iostream>\nstd::uint32_t recover(std::uint32_t);\nint main(){\n",
            );
            let mut expected = Vec::new();
            for input in inputs {
                code.push_str(&format!("std::cout<<recover({input}u)<<'\\n';\n"));
                expected.push((input.rotate_left(5) ^ 0x6d2b79f5u32).wrapping_add(0x013579bdu32).to_string());
            }
            code.push_str("}\n");
            (code, expected)
        }
        Kind::Reasoning => (String::new(), Vec::new()),
    }
}
pub fn validate_plan(bytes: &[u8]) -> Result<Value> {
    let plan: Value = serde_json::from_slice(bytes)?;
    let object = plan.as_object().ok_or_else(|| anyhow::anyhow!("plan must be an object"))?;
    ensure!(
        object.len() == 3 && ["starts", "makespan", "critical_path"].iter().all(|key| object.contains_key(*key)),
        "exact plan fields required"
    );
    let starts = plan["starts"].as_object().ok_or_else(|| anyhow::anyhow!("starts must be an object"))?;
    let names = ["A", "B", "C", "D"];
    ensure!(starts.len() == 4 && names.iter().all(|name| starts.contains_key(*name)), "exact job starts required");
    let mut times = [0i64; 4];
    for (index, name) in names.iter().enumerate() {
        times[index] = starts[*name].as_i64().ok_or_else(|| anyhow::anyhow!("integer starts required"))?;
        ensure!((0..=8).contains(&times[index]), "start outside 0..8");
    }
    let valid = |s: [i64; 4]| {
        let d = [2, 3, 2, 1];
        s[2] >= s[0] + 2
            && s[3] >= s[1] + 3
            && s[3] >= s[2] + 2
            && (0..12).all(|t| (0..4).filter(|i| s[*i] <= t && t < s[*i] + d[*i]).count() <= 2)
    };
    ensure!(valid(times), "precedence or simultaneous-worker constraint violated");
    let makespan = (0..4).map(|i| times[i] + [2, 3, 2, 1][i]).max().unwrap();
    ensure!(plan["makespan"].as_i64() == Some(makespan), "declared makespan differs from actual schedule");
    let mut optimum = i64::MAX;
    for a in 0..=8 {
        for b in 0..=8 {
            for c in 0..=8 {
                for d in 0..=8 {
                    let s = [a, b, c, d];
                    if valid(s) {
                        optimum = optimum.min((0..4).map(|i| s[i] + [2, 3, 2, 1][i]).max().unwrap());
                    }
                }
            }
        }
    }
    ensure!(makespan == optimum, "schedule is feasible but not minimum-makespan");
    let path = plan["critical_path"].as_array().ok_or_else(|| anyhow::anyhow!("critical_path must be an array"))?;
    ensure!(!path.is_empty() && path.len() <= 4, "bounded nonempty critical path required");
    let mut ids = Vec::new();
    for item in path {
        let name = item.as_str().ok_or_else(|| anyhow::anyhow!("critical path IDs must be strings"))?;
        ids.push(names.iter().position(|n| *n == name).ok_or_else(|| anyhow::anyhow!("unknown critical path job"))?);
    }
    ensure!(
        ids.windows(2).all(|pair| matches!((pair[0], pair[1]), (0, 2) | (1, 3) | (2, 3))),
        "critical path contains a non-dependency edge"
    );
    ensure!(ids.iter().map(|i| [2, 3, 2, 1][*i]).sum::<i64>() == makespan, "critical path does not establish makespan");
    Ok(json!({"valid":true,"makespan":makespan,"exhaustive_optimum":optimum,"independent_schedule_search":true}))
}
