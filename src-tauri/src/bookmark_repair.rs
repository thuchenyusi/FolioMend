use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeMap, BTreeSet};

fn link(node: &Dictionary, key: &[u8]) -> Result<Option<ObjectId>, String> {
    match node.get(key) {
        Ok(Object::Reference(id)) => Ok(Some(*id)),
        // Inline items may contain real titles and cannot be silently dropped.
        Ok(Object::Dictionary(_) | Object::Array(_) | Object::Stream(_)) => {
            Err("书签链接包含无法可靠恢复的内嵌对象".into())
        }
        _ => Ok(None),
    }
}

fn endpoint(
    doc: &Document,
    id: Option<ObjectId>,
    ancestors: &BTreeSet<ObjectId>,
) -> Option<ObjectId> {
    id.filter(|id| !ancestors.contains(id) && doc.get_object(*id).is_ok())
}

fn walk(
    doc: &Document,
    mut current: Option<ObjectId>,
    key: &[u8],
    ancestors: &BTreeSet<ObjectId>,
) -> Result<Vec<ObjectId>, String> {
    let mut nodes = Vec::new();
    let mut seen = BTreeSet::new();
    while let Some(id) = current {
        if ancestors.contains(&id) || !seen.insert(id) {
            break;
        }
        let item = doc
            .get_dictionary(id)
            .map_err(|_| "书签条目缺失或不是字典")?;
        if item.get(b"Title").and_then(Object::as_str).is_err() {
            return Err("书签标题缺失或无法解析".into());
        }
        nodes.push(id);
        current = link(item, key)?;
    }
    Ok(nodes)
}

fn siblings(
    doc: &Document,
    node: &Dictionary,
    ancestors: &BTreeSet<ObjectId>,
) -> Result<Vec<ObjectId>, String> {
    let first = endpoint(doc, link(node, b"First")?, ancestors);
    let last = endpoint(doc, link(node, b"Last")?, ancestors);
    let mut forward = walk(doc, first, b"Next", ancestors)?;
    let mut backward = walk(doc, last, b"Prev", ancestors)?;
    backward.reverse();
    let known = forward.iter().copied().collect::<BTreeSet<_>>();
    if backward.iter().any(|id| !known.contains(id)) {
        // Merge missing items from the reverse chain. Shared anchors must have
        // the same order; otherwise the original reading order is ambiguous.
        let anchors = backward
            .iter()
            .filter_map(|id| forward.iter().position(|f| f == id))
            .collect::<Vec<_>>();
        if anchors.windows(2).any(|w| w[0] >= w[1]) {
            return Err("书签顺序存在冲突，无法可靠恢复".into());
        }
        let mut gap = Vec::new();
        for id in backward {
            if let Some(index) = forward.iter().position(|f| *f == id) {
                forward.splice(index..index, gap.drain(..));
            } else {
                gap.push(id);
            }
        }
        forward.extend(gap);
    }
    if forward.is_empty()
        && (node.get(b"First").is_ok()
            || node.get(b"Last").is_ok()
            || node.get(b"Count").and_then(Object::as_i64).unwrap_or(0) != 0)
    {
        return Err("无法恢复书签子列表".into());
    }
    let members = forward.iter().copied().collect::<BTreeSet<_>>();
    // Account for both directions, including pointers not used by the chosen
    // chain. Never discard an additional title simply to make the tree valid.
    for id in &forward {
        let item = doc.get_dictionary(*id).map_err(|_| "书签条目无法解析")?;
        for key in [b"Next".as_slice(), b"Prev"] {
            if let Some(other) = link(item, key)? {
                if !ancestors.contains(&other)
                    && !members.contains(&other)
                    && doc.get_object(other).is_ok()
                {
                    return Err("存在无法确定层级或顺序的书签条目".into());
                }
            }
        }
    }
    Ok(forward)
}

fn set_link(node: &mut Dictionary, key: &'static str, value: Option<ObjectId>) {
    if let Some(id) = value {
        node.set(key, id);
    } else {
        node.remove(key.as_bytes());
    }
}

/// Build a repair plan containing only outline dictionaries. Titles, actions,
/// destinations, style, and unrelated PDF objects remain untouched.
pub fn repair_structure(doc: &mut Document) -> Result<usize, String> {
    let outlines = doc
        .catalog()
        .map_err(|_| "文档目录无法解析")?
        .get(b"Outlines")
        .map_err(|_| "书签入口缺失")?
        .clone();
    let (root_id, root, inline) = match &outlines {
        Object::Reference(id) => (
            *id,
            doc.get_dictionary(*id)
                .map_err(|_| "书签根节点无法解析")?
                .clone(),
            false,
        ),
        Object::Dictionary(root) => (
            (doc.max_id.checked_add(1).ok_or("对象编号已满")?, 0),
            root.clone(),
            true,
        ),
        _ => return Err("书签根节点不是字典".into()),
    };
    if root.get(b"Title").is_ok() {
        return Err("书签根节点含标题，无法确认原始层级".into());
    }
    let mut plan = BTreeMap::from([(root_id, root)]);
    let mut ownership = BTreeSet::from([root_id]);
    let mut pending = vec![(root_id, BTreeSet::from([root_id]))];
    let mut groups = Vec::new();
    while let Some((parent, ancestors)) = pending.pop() {
        let children = siblings(doc, &plan[&parent], &ancestors)?;
        for (index, &id) in children.iter().enumerate() {
            if !ownership.insert(id) {
                return Err("同一书签属于多个层级，无法可靠恢复".into());
            }
            let mut item = doc
                .get_dictionary(id)
                .map_err(|_| "书签条目无法解析")?
                .clone();
            item.set("Parent", parent);
            set_link(&mut item, "Prev", index.checked_sub(1).map(|i| children[i]));
            set_link(&mut item, "Next", children.get(index + 1).copied());
            plan.insert(id, item);
            let mut path = ancestors.clone();
            path.insert(id);
            if path.len() > 512 {
                return Err("书签层级过深，无法可靠恢复".into());
            }
            pending.push((id, path));
        }
        let parent_node = plan.get_mut(&parent).unwrap();
        set_link(parent_node, "First", children.first().copied());
        set_link(parent_node, "Last", children.last().copied());
        groups.push((parent, children));
    }
    let count = ownership.len() - 1;
    if count == 0 {
        return Err("未找到可恢复的书签条目".into());
    }
    for key in [b"Prev".as_slice(), b"Next"] {
        if let Some(id) = link(&plan[&root_id], key)? {
            if !ownership.contains(&id)
                && doc
                    .get_dictionary(id)
                    .is_ok_and(|node| node.get(b"Title").is_ok())
            {
                return Err("根节点链接到无法确定层级的书签条目".into());
            }
        }
    }
    let mut visible = BTreeMap::<ObjectId, i64>::new();
    for (id, children) in groups.iter().rev() {
        let total = children
            .iter()
            .map(|child| {
                let open = plan[child]
                    .get(b"Count")
                    .and_then(Object::as_i64)
                    .unwrap_or(0)
                    >= 0;
                1 + if open { visible[child] } else { 0 }
            })
            .sum::<i64>();
        visible.insert(*id, total);
        let item = plan.get_mut(id).unwrap();
        let closed = *id != root_id && item.get(b"Count").and_then(Object::as_i64).unwrap_or(0) < 0;
        if children.is_empty() && *id != root_id {
            item.remove(b"Count");
        } else {
            item.set("Count", if closed { -total } else { total });
        }
    }
    let root = plan.get_mut(&root_id).unwrap();
    root.set("Type", "Outlines");
    for key in [b"Parent".as_slice(), b"Prev", b"Next"] {
        root.remove(key);
    }
    let old_max = doc.max_id;
    let originals = plan
        .into_iter()
        .map(|(id, dict)| (id, doc.objects.insert(id, Object::Dictionary(dict))))
        .collect::<Vec<_>>();
    if inline {
        doc.max_id = root_id.0;
        doc.catalog_mut().unwrap().set("Outlines", root_id);
    }
    if crate::pdf_processor::inspect_bookmarks(doc).abnormal {
        for (id, old) in originals {
            if let Some(old) = old {
                doc.objects.insert(id, old);
            } else {
                doc.objects.remove(&id);
            }
        }
        doc.max_id = old_max;
        doc.catalog_mut().unwrap().set("Outlines", outlines);
        return Err("修复后书签结构仍不完整".into());
    }
    Ok(count)
}

/// Count visible descendants, accounting for each item's expanded/collapsed
/// state, while also detecting shared items and cycles across child lists.
pub fn counts_are_valid(doc: &Document, root: ObjectId) -> bool {
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    let mut groups = Vec::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            return false;
        }
        let Ok(node) = doc.get_dictionary(id) else {
            return false;
        };
        let mut children = Vec::new();
        let mut siblings = BTreeSet::new();
        let mut child = node.get(b"First").and_then(Object::as_reference).ok();
        while let Some(id) = child {
            if !siblings.insert(id) {
                return false;
            }
            children.push(id);
            let Ok(item) = doc.get_dictionary(id) else {
                return false;
            };
            child = item.get(b"Next").and_then(Object::as_reference).ok();
        }
        pending.extend(children.iter().copied());
        groups.push((id, children));
    }
    let mut visible = BTreeMap::<ObjectId, i64>::new();
    for (id, children) in groups.iter().rev() {
        let expected = children
            .iter()
            .map(|child| {
                let open = doc
                    .get_dictionary(*child)
                    .unwrap()
                    .get(b"Count")
                    .and_then(Object::as_i64)
                    .unwrap_or(0)
                    >= 0;
                1 + if open { visible[child] } else { 0 }
            })
            .sum::<i64>();
        let count = doc.get_dictionary(*id).unwrap().get(b"Count");
        match count {
            Ok(value) => {
                let Ok(actual) = value.as_i64() else {
                    return false;
                };
                if actual.checked_abs() != Some(expected) || (*id == root && actual < 0) {
                    return false;
                }
            }
            Err(_) if expected > 0 => return false,
            Err(_) => {}
        }
        visible.insert(*id, expected);
    }
    true
}
