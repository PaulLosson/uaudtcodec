"""OPC-UA XML type dictionary parser."""

from bs4 import BeautifulSoup

from ._utils import sanitize_name


class StructuredTypeParser:
    def __init__(self, dict_value):
        """Initialize the parser with the XML dictionary content."""
        self.structured_types = []
        self.enumeration_types = []
        self._parse_types_dictionary(dict_value)

    def _parse_types_dictionary(self, dict_value):
        """Parse the structured types and enumerations from XML content."""
        soup = BeautifulSoup(dict_value, 'xml')

        for structured_type in soup.find_all('opc:StructuredType'):
            type_name = sanitize_name(structured_type.get('Name'))

            # Collect length field names so we can skip them
            length_fields = set()
            for field in structured_type.find_all('opc:Field'):
                lf = field.get('LengthField')
                if lf:
                    length_fields.add(lf)

            field_list = []
            for field in structured_type.find_all('opc:Field'):
                raw_name = field.get('Name')
                if raw_name in length_fields:
                    continue
                field_name = sanitize_name(raw_name)
                field_type = field.get('TypeName').split(':')[-1]
                field_list.append({
                    "Name": field_name,
                    "Type": field_type,
                    "IsArray": bool(field.get('LengthField')),
                })

            self.structured_types.append({
                "StructuredTypeName": type_name,
                "Fields": field_list,
            })

        for enumeration_type in soup.find_all('opc:EnumeratedType'):
            field_list = []
            for field in enumeration_type.find_all('opc:EnumeratedValue'):
                field_list.append({
                    "Name": field.get('Name'),
                    "Value": field.get('Value').split(':')[-1],
                })

            self.enumeration_types.append({
                "Name": enumeration_type.get('Name'),
                "Fields": field_list,
            })

    def get_structured_types(self):
        return self.structured_types

    def get_enumeration_types(self):
        return self.enumeration_types

    def find_structured_type(self, structure_name):
        return next(
            (st for st in self.structured_types if st["StructuredTypeName"] == structure_name),
            None,
        )
