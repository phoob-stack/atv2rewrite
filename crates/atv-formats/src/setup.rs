//! The game's `Setup/*.xml` rule files (riders, vehicles, tracks, front-end
//! screens). They are flat: one element per entry under `<Root>`, each with
//! attributes and a few attribute-only children (`<Characteristics .../>`).
//! An entry's attributes and its children's are gathered into one map.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub name: String,
    pub attrs: HashMap<String, String>,
}

impl Entry {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(String::as_str)
    }

    pub fn num(&self, key: &str) -> Option<f32> {
        self.get(key)?.trim().parse().ok()
    }
}

/// Every top-level entry, in file order.
pub fn entries(xml: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut depth = 0i32;
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if tag.starts_with('/') {
            depth -= 1;
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, attr_text) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let attrs = parse_attrs(attr_text);
        // depth 0 is <Root>; its children are the entries.
        if depth == 1 {
            out.push(Entry { name: name.to_string(), attrs });
        } else if depth > 1
            && let Some(e) = out.last_mut()
        {
            for (k, v) in attrs {
                e.attrs.entry(k).or_insert(v);
            }
        }
        if !self_closing {
            depth += 1;
        }
    }
    out
}

pub(crate) fn parse_attrs(mut s: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    while let Some(eq) = s.find('=') {
        let key = s[..eq].trim().to_string();
        let after = s[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let Some(close) = after[1..].find(q) else { break };
        m.insert(key, after[1..1 + close].to_string());
        s = &after[close + 2..];
    }
    m
}

#[cfg(test)]
mod tests {
    #[test]
    fn flat_entries_with_children() {
        let xml = r#"<Root FileVersion="1.0">
   <Doug_Gust mModelFilename="Male10" mWeight="162">
      <Characteristics mAbility="4" mTechnique="5"/>
   </Doug_Gust>
   <Bandit mAvailable="1"/>
</Root>"#;
        let e = super::entries(xml);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "Doug_Gust");
        assert_eq!(e[0].get("mModelFilename"), Some("Male10"));
        assert_eq!(e[0].num("mTechnique"), Some(5.0));
        assert_eq!(e[1].get("mAvailable"), Some("1"));
    }
}
