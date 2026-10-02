/// A page of todos.
pub type TodoPage = model::Page<Todo>;
model::Page! {
    TodoPage, "A page of todos.", Todo
}
