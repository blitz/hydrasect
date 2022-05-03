// SPDX-FileCopyrightText: 2022 Alyssa Ross <hi@alyssa.is>
// SPDX-License-Identifier: EUPL-1.2

#include <stdio.h>
#include <unistd.h>

int tmpfd(void)
{
	int fd = -1;
	FILE *f = tmpfile();
	if (!f)
		return -1;
	if ((fd = fileno(f)) != -1)
		fd = dup(fd);
	fclose(f);
	return fd;
}
