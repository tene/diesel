use std::marker::PhantomData;

use backend::{Backend, SupportsDefaultKeyword};
use expression::{AppearsOnTable, Expression, Grouped};
use result::QueryResult;
use query_builder::{AstPass, QueryFragment, UndecoratedInsertRecord};
use query_source::{Column, Table};
#[cfg(feature = "sqlite")]
use sqlite::Sqlite;

/// Represents that a structure can be used to insert a new row into the
/// database. This is automatically implemented for `&[T]` and `&Vec<T>` for
/// inserting more than one record.
///
/// ### Deriving
///
/// This trait can be automatically derived by adding  `#[derive(Insertable)]`
/// to your struct. Structs which derive this trait must also be annotated
/// with `#[table_name = "some_table_name"]`. If the field name of your
/// struct differs from the name of the column, you can annotate the field
/// with `#[column_name = "some_column_name"]`.
pub trait Insertable<T> {
    type Values;

    fn values(self) -> Self::Values;
}

pub trait CanInsertInSingleQuery<DB: Backend> {
    fn rows_to_insert(&self) -> usize;
}

impl<'a, T, DB> CanInsertInSingleQuery<DB> for &'a T
where
    T: ?Sized + CanInsertInSingleQuery<DB>,
    DB: Backend,
{
    fn rows_to_insert(&self) -> usize {
        (*self).rows_to_insert()
    }
}

impl<'a, T, Tab, DB> CanInsertInSingleQuery<DB> for BatchInsert<'a, T, Tab>
where
    DB: Backend + SupportsDefaultKeyword,
{
    fn rows_to_insert(&self) -> usize {
        self.records.len()
    }
}

impl<T, U, DB> CanInsertInSingleQuery<DB> for ColumnInsertValue<T, U>
where
    DB: Backend,
{
    fn rows_to_insert(&self) -> usize {
        1
    }
}

impl<T, DB> CanInsertInSingleQuery<DB> for Grouped<T>
where
    DB: Backend,
    T: CanInsertInSingleQuery<DB>,
{
    fn rows_to_insert(&self) -> usize {
        self.0.rows_to_insert()
    }
}

pub trait InsertValues<T: Table, DB: Backend>: QueryFragment<DB> {
    fn column_names(&self, out: AstPass<DB>) -> QueryResult<()>;
}

#[derive(Debug, Copy, Clone)]
#[doc(hidden)]
pub enum ColumnInsertValue<Col, Expr> {
    Expression(Col, Expr),
    Default,
}

impl<Col, Expr> Default for ColumnInsertValue<Col, Expr> {
    fn default() -> Self {
        ColumnInsertValue::Default
    }
}

impl<Col, Expr, DB> InsertValues<Col::Table, DB> for ColumnInsertValue<Col, Expr>
where
    DB: Backend + SupportsDefaultKeyword,
    Col: Column,
    Expr: Expression<SqlType = Col::SqlType> + AppearsOnTable<()>,
    Self: QueryFragment<DB>,
{
    fn column_names(&self, mut out: AstPass<DB>) -> QueryResult<()> {
        out.push_identifier(Col::NAME)?;
        Ok(())
    }
}

impl<Col, Expr, DB> QueryFragment<DB> for ColumnInsertValue<Col, Expr>
where
    DB: Backend + SupportsDefaultKeyword,
    Expr: QueryFragment<DB>,
{
    fn walk_ast(&self, mut out: AstPass<DB>) -> QueryResult<()> {
        if let ColumnInsertValue::Expression(_, ref value) = *self {
            value.walk_ast(out.reborrow())?;
        } else {
            out.push_sql("DEFAULT");
        }
        Ok(())
    }
}

#[cfg(feature = "sqlite")]
impl<Col, Expr> InsertValues<Col::Table, Sqlite> for ColumnInsertValue<Col, Expr>
where
    Col: Column,
    Expr: Expression<SqlType = Col::SqlType> + AppearsOnTable<()>,
    Self: QueryFragment<Sqlite>,
{
    fn column_names(&self, mut out: AstPass<Sqlite>) -> QueryResult<()> {
        if let ColumnInsertValue::Expression(..) = *self {
            out.push_identifier(Col::NAME)?;
        }
        Ok(())
    }
}

#[cfg(feature = "sqlite")]
impl<Col, Expr> QueryFragment<Sqlite> for ColumnInsertValue<Col, Expr>
where
    Expr: QueryFragment<Sqlite>,
{
    fn walk_ast(&self, mut out: AstPass<Sqlite>) -> QueryResult<()> {
        if let ColumnInsertValue::Expression(_, ref value) = *self {
            value.walk_ast(out.reborrow())?;
        }
        Ok(())
    }
}

impl<'a, T, Tab> Insertable<Tab> for &'a [T]
where
    &'a T: UndecoratedInsertRecord<Tab>,
{
    type Values = BatchInsert<'a, T, Tab>;

    fn values(self) -> Self::Values {
        BatchInsert {
            records: self,
            _marker: PhantomData,
        }
    }
}

impl<'a, T, Tab> Insertable<Tab> for &'a Vec<T>
where
    &'a [T]: Insertable<Tab>,
{
    type Values = <&'a [T] as Insertable<Tab>>::Values;

    fn values(self) -> Self::Values {
        (&**self).values()
    }
}

impl<T, Tab> Insertable<Tab> for Option<T>
where
    T: Insertable<Tab>,
    T::Values: Default,
{
    type Values = T::Values;

    fn values(self) -> Self::Values {
        self.map(Insertable::values).unwrap_or_default()
    }
}

impl<'a, T, Tab> Insertable<Tab> for &'a Option<T>
where
    Option<&'a T>: Insertable<Tab>,
{
    type Values = <Option<&'a T> as Insertable<Tab>>::Values;

    fn values(self) -> Self::Values {
        self.as_ref().values()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BatchInsert<'a, T: 'a, Tab> {
    records: &'a [T],
    _marker: PhantomData<Tab>,
}

impl<'a, T, Tab> IntoIterator for BatchInsert<'a, T, Tab> {
    type Item = &'a T;
    type IntoIter = <&'a [T] as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.records.into_iter()
    }
}

impl<'a, T, Tab, DB> InsertValues<Tab, DB> for BatchInsert<'a, T, Tab>
where
    Tab: Table,
    DB: Backend + SupportsDefaultKeyword,
    &'a T: Insertable<Tab>,
    <&'a T as Insertable<Tab>>::Values: InsertValues<Tab, DB>,
{
    fn column_names(&self, out: AstPass<DB>) -> QueryResult<()> {
        self.records
            .get(0)
            .expect("Tried to read column names from empty list of rows")
            .values()
            .column_names(out)
    }
}

impl<'a, T, Tab, DB> QueryFragment<DB> for BatchInsert<'a, T, Tab>
where
    DB: Backend + SupportsDefaultKeyword,
    &'a T: Insertable<Tab>,
    <&'a T as Insertable<Tab>>::Values: QueryFragment<DB>,
{
    fn walk_ast(&self, mut out: AstPass<DB>) -> QueryResult<()> {
        for (i, record) in self.records.iter().enumerate() {
            if i != 0 {
                out.push_sql(", ");
            }
            record.values().walk_ast(out.reborrow())?;
        }
        Ok(())
    }
}
